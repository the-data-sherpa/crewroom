#!/usr/bin/env python3
"""Discover and run the three FOR-12 native verification recipes.

check/run never install or rebuild. Build is a separate, explicit operation.
Evidence is private and retained, including failed and interrupted runs.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import signal
import stat
import subprocess
import sys
import time
import uuid

import test_processes as processes
from docker_engine import DockerEngineError, docker_socket, require_docker_engine

REPO = Path(__file__).resolve().parents[1]
KEYS = ('steering-reconnect', 'pause-resume', 'readiness-invalidation')
ENTRY = 'scripts/rust/feature-recipes.test.ts'
NATIVE_SELECTOR = 'workbench::journey_tests::feature_recipes::native_feature_recipe_control'
MANIFEST = REPO / 'docs/testing/feature-map/manifest.json'
BUILD = REPO / '.crewroom/feature-verification/build.json'
BINARY_NAMES = ('crewroomd', 'crewroom-host', 'crewroom-process', 'crewroom',
                'examples/workflow-fixture', 'examples/container-fixture')
SOURCE_PATHS = ('Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', '.cargo', 'crates', 'vendor')
EXIT = {'passed': 0, 'ready': 0, 'failed': 1, 'unavailable': 2, 'incomplete': 3, 'stale': 4}


class Problem(Exception):
    def __init__(self, outcome, reason):
        self.outcome, self.reason = outcome, reason
        super().__init__(reason)


def sha(path):
    digest = hashlib.sha256()
    with Path(path).open('rb') as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b''):
            digest.update(chunk)
    return digest.hexdigest()


def object_sha(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(',', ':')).encode()).hexdigest()


def write_json(path, value):
    path = Path(path)
    temporary = path.with_name(path.name + '.new')
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'w') as out:
        json.dump(value, out, indent=2)
        out.write('\n')
        out.flush()
        os.fsync(out.fileno())
    os.replace(temporary, path)


def capture(command, *, env=None, timeout=20, input=None):
    try:
        result = subprocess.run(command, cwd=REPO, env=env, input=input,
                                text=True, capture_output=True, timeout=timeout)
    except FileNotFoundError as error:
        raise Problem('unavailable', f'Missing executable: {command[0]}') from error
    except subprocess.TimeoutExpired as error:
        raise Problem('incomplete', f'Prerequisite command timed out: {command!r}') from error
    if result.returncode:
        raise Problem('unavailable', f'Command failed ({result.returncode}): {command!r}: {result.stderr.strip()[:800]}')
    return result.stdout.strip()


def relative_file(value):
    if not isinstance(value, str) or Path(value).is_absolute() or '..' in Path(value).parts:
        raise Problem('stale', f'Invalid repository file reference: {value!r}')
    path = REPO / value
    if not path.is_file() or not path.resolve().is_relative_to(REPO):
        raise Problem('stale', f'Referenced file is missing or outside checkout: {value}')
    return path


def load_manifest(path):
    try:
        value = json.loads(path.read_text())
        recipes = value['recipes']
        if value['schemaVersion'] != 1 or {r['key'] for r in recipes} != set(KEYS) or len(recipes) != 3:
            raise ValueError('Expected schemaVersion 1 and exactly the three FOR-12 recipes.')
        ids = set(re.findall(r'"id"\s*:\s*"([A-Z][A-Z0-9]*-\d+)"', (REPO / 'docs/testing/suite.json').read_text()))
        ids.update(re.findall(r'"id"\s*:\s*"([A-Z][A-Z0-9]*-\d+)"', (REPO / 'docs/testing/scenarios.json').read_text()))
        for recipe in recipes:
            for field in ('title', 'aliases', 'recipePath', 'cases', 'entryPoint', 'testName',
                          'nativeSelector', 'sourcePaths', 'assertions', 'requiresDocker', 'timeoutSeconds'):
                if field not in recipe:
                    raise ValueError(f'{recipe["key"]}: missing {field}')
            if recipe['entryPoint'] != ENTRY or recipe['testName'] != recipe['key']:
                raise ValueError('Only the fixed native recipe entry point and canonical test names are supported.')
            if not recipe['cases'] or not set(recipe['cases']) <= ids:
                raise ValueError(f'{recipe["key"]}: unknown acceptance case reference')
            if not recipe['assertions'] or len(set(recipe['assertions'])) != len(recipe['assertions']):
                raise ValueError('Required assertion IDs must be nonempty and unique.')
            if not isinstance(recipe['requiresDocker'], bool) or not 30 <= recipe['timeoutSeconds'] <= 600:
                raise ValueError('Invalid prerequisite or bounded deadline.')
            if recipe['nativeSelector'] != NATIVE_SELECTOR:
                raise ValueError('Native selector differs from the fixed feature recipe driver; update the manifest/driver binding together.')
            for name in [recipe['recipePath'], recipe['entryPoint'], *recipe['sourcePaths']]:
                relative_file(name)
            source = relative_file(recipe['entryPoint']).read_text()
            count = len(re.findall(r'\btest\s*\(\s*[\'\"]' + re.escape(recipe['testName']) + r'[\'\"]', source))
            if count != 1:
                raise ValueError(f'{recipe["key"]}: expected exactly one source test entry, found {count}')
        return value
    except (OSError, KeyError, TypeError, ValueError) as error:
        raise Problem('stale', f'Manifest needs correction: {error}') from error


def source_identity():
    raw = capture(['git', 'ls-files', '-z', '--cached', '--others', '--exclude-standard', '--', *SOURCE_PATHS])
    files = {name: sha(REPO / name) if (REPO / name).is_file() else None
             for name in sorted(set(raw.split('\0')) - {''})}
    if not files:
        raise Problem('stale', 'No native source files found for executable binding.')
    return {'sha256': object_sha(files), 'files': files}


def revision():
    return {'commit': capture(['git', 'rev-parse', 'HEAD']),
            'dirtyDiffSha256': hashlib.sha256(subprocess.check_output(
                ['git', 'diff', 'HEAD', '--binary'], cwd=REPO)).hexdigest(),
            'status': capture(['git', 'status', '--porcelain'])}


def platform_identity():
    return {'system': platform.system(), 'release': platform.release(), 'machine': platform.machine(),
            'python': platform.python_version(), 'nativeMode': 'GPUI test harness; no OS-input certification',
            'display': {'wayland': bool(os.environ.get('WAYLAND_DISPLAY')), 'x11': bool(os.environ.get('DISPLAY'))}}


def bound_build():
    if not BUILD.is_file():
        raise Problem('unavailable', 'No verified build receipt. Run verify-feature.py build --evidence-dir <fresh directory>.')
    try:
        receipt = processes.private_json(BUILD)
        if receipt['outcome'] != 'passed' or receipt['repo'] != str(REPO):
            raise ValueError('Build did not finish successfully in this checkout.')
        if receipt['source']['sha256'] != source_identity()['sha256']:
            raise ValueError('Native source changed since the verified build.')
        binaries = receipt['binaries']
        if set(binaries) != {*BINARY_NAMES, 'nativeTest'}:
            raise ValueError('Incomplete binary identities.')
        for name, item in binaries.items():
            path = Path(item['path'])
            if not path.resolve().is_relative_to(REPO / 'target/debug') or not path.is_file() or not os.access(path, os.X_OK):
                raise ValueError(f'Executable is missing/outside the debug build: {name}')
            if sha(path) != item['sha256']:
                raise ValueError(f'Executable content no longer matches its build receipt: {name}')
        return receipt
    except (OSError, KeyError, TypeError, ValueError) as error:
        raise Problem('stale', f'Executable/source binding is stale: {error} Rebuild explicitly.') from error


def preflight(recipe, manifest):
    facts = {'recipe': recipe['key'], 'manifestSha256': sha(manifest), 'platform': platform_identity(), 'probes': []}
    def probe(args):
        output = capture(args)
        facts['probes'].append({'command': args, 'output': output})
        return output
    if platform.system() != 'Linux':
        raise Problem('unavailable', 'This recipe surface is currently verified only on Linux source/debug.')
    version = probe(['node', '--version'])
    if not re.match(r'v(2[4-9]|[3-9][0-9])\.', version):
        raise Problem('unavailable', f'Node >=24 is required; found {version}. Select an installed supported Node explicitly.')
    probe(['node', '--import', 'tsx', '-e', 'process.stdout.write("tsx available")'])
    probe(['git', '--version'])
    probe([os.environ.get('CREWROOM_TMUX', '/usr/bin/tmux'), '-V'])
    if not Path('/usr/bin/lsof').is_file():
        raise Problem('unavailable', 'Ownership checks require /usr/bin/lsof.')
    build = bound_build()
    listing = probe([build['binaries']['nativeTest']['path'], recipe['nativeSelector'], '--list'])
    names = [line[:-6] for line in listing.splitlines() if line.endswith(': test')]
    if names != [recipe['nativeSelector']]:
        raise Problem('stale', f'Expected exactly one native test {recipe["nativeSelector"]!r}; found {names!r}.')
    facts['expectedNativeTestCount'] = 1
    if recipe['requiresDocker']:
        image = os.environ.get('CREWROOM_CHECK_IMAGE')
        if not image or not re.fullmatch(r'sha256:[a-f0-9]{64}', image):
            raise Problem('unavailable', 'Set CREWROOM_CHECK_IMAGE to a pinned, already-local sha256 image ID with /bin/sh.')
        try:
            socket = require_docker_engine()
        except DockerEngineError as error:
            raise Problem('unavailable', str(error))
        facts['dockerSocket'] = socket
        if probe(['docker', '--host', f'unix://{socket}', 'image', 'inspect', image, '--format', '{{.Id}}']) != image:
            raise Problem('unavailable', 'Local check image did not match the requested pinned ID.')
        facts['checkImage'] = image
    facts['buildReceiptSha256'] = sha(BUILD)
    facts['buildReceipt'] = str(BUILD)
    facts['buildToolchain'] = build.get('toolchain', {})
    facts['recipeCommit'] = capture(['git', 'log', '-1', '--format=%H', '--', recipe['recipePath']]) or None
    facts['binaries'] = build['binaries']
    facts['sourceSha256'] = build['source']['sha256']
    facts['recipeSources'] = {name: sha(relative_file(name)) for name in sorted(set([
        recipe['recipePath'], recipe['entryPoint'], *recipe['sourcePaths'], 'scripts/verify-feature.py', 'scripts/test_processes.py', 'scripts/docker_engine.py']))}
    return facts


def new_run(directory, kind, key=None):
    root = processes.fixture_root(directory, REPO)
    if root.exists():
        raise Problem('unavailable', 'Evidence directory already exists; choose a fresh directory. Prior evidence is never overwritten.')
    root.mkdir(parents=True, mode=0o700)
    for name in ('fixtures', 'resources'):
        (root / name).mkdir(mode=0o700)
    ident = str(uuid.uuid4())
    receipt = {'schemaVersion': 1, 'kind': kind, 'id': ident, 'repo': str(REPO), 'recipe': key,
               'evidenceDirectory': str(root), 'startedAt': time.time(), 'outcome': 'incomplete',
               'reason': 'Execution has not completed; use cleanup with this receipt after an interruption.',
               'commands': [], 'platform': platform_identity()}
    path = root / 'receipt.json'
    # Publish scope and receipt before any workload can launch.
    write_json(path, receipt)
    record = {'id': ident, 'repo': str(REPO), 'label': f'FOR-12 {kind} {key or ""}',
              'created': time.time(), 'roots': [str(root)], 'featureReceipt': str(path)}
    write_json(processes.registry(REPO, create=True) / f'{ident}.json', record)
    return root, receipt


def run_command(command, root, receipt, env, timeout, label):
    entry = {'command': command, 'startedAt': time.time(), 'timeoutSeconds': timeout,
             'log': str(root / f'{len(receipt["commands"]):02d}-{label}.log'), 'state': 'launch-intent'}
    receipt['commands'].append(entry)
    write_json(root / 'receipt.json', receipt)
    with open(entry['log'], 'x', opener=lambda p, flags: os.open(p, flags, 0o600)) as output:
        child = subprocess.Popen(command, cwd=REPO, env=env, stdout=output, stderr=subprocess.STDOUT, start_new_session=True)
        entry.update(pid=child.pid, state='running')
        write_json(root / 'receipt.json', receipt)
        try:
            code = child.wait(timeout=timeout)
        except (subprocess.TimeoutExpired, KeyboardInterrupt) as error:
            entry['interruption'] = type(error).__name__
            child.terminate()
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait(timeout=5)
            raise Problem('incomplete', f'{label} interrupted or exceeded its {timeout}s deadline.') from error
        finally:
            entry.update(endedAt=time.time(), exitCode=child.poll(), state='finished')
            write_json(root / 'receipt.json', receipt)
    return code, Path(entry['log'])


def env_for(receipt):
    return dict(os.environ, **{processes.RUN_KEY: receipt['id'], processes.REPO_KEY: str(REPO)})


def owned_cleanup(path):
    receipt = processes.private_json(path)
    root = Path(receipt['evidenceDirectory'])
    if receipt.get('schemaVersion') != 1 or receipt.get('repo') != str(REPO) or path != root / 'receipt.json':
        raise Problem('incomplete', 'Foreign or moved cleanup receipt refused.')
    if processes.fixture_root(root, REPO) != root or root.is_symlink() or root.stat().st_mode & 0o077:
        raise Problem('incomplete', 'Cleanup root must be the original private fixture directory.')
    runs = processes.load_runs(REPO)
    record = runs.get(receipt['id'])
    if not record or record.get('featureReceipt') != str(path) or record['roots'] != [root]:
        raise Problem('incomplete', 'Cleanup receipt is not linked to this exact registered test run.')
    report = {'startedAt': time.time(), 'resources': [], 'remaining': [], 'fallback': False}
    helper_item = receipt.get('preflight', {}).get('binaries', {}).get('crewroom-process')
    if helper_item:
        helper = REPO / 'target/debug/crewroom-process'
        if str(helper) != helper_item['path'] or sha(helper) != helper_item['sha256']:
            raise Problem('incomplete', 'Process helper changed; refusing unbound cleanup executable.')
        # Daemon shutdown gets the first chance to stop hosts, runners and tmux.
        candidates = sorted(root.rglob('launch.json'), key=lambda p: (not p.parent.name.startswith('daemon-'), str(p)))
        for launch in candidates:
            owned = launch.parent
            if not owned.resolve().is_relative_to(root) or any(p.is_symlink() for p in (owned, launch)):
                raise Problem('incomplete', 'Symlinked owned process resource refused.')
            value = json.loads(capture([str(helper), 'inspect', str(owned)]))
            event = {'root': str(owned), 'before': value}
            if value.get('state') == 'live':
                report['fallback'] = True
                event['after'] = json.loads(capture([str(helper), 'stop', str(owned)], timeout=30))
            else:
                event['after'] = value
            if event['after'].get('state') != 'stopped':
                report['remaining'].append(str(owned))
            # Retain process identities, never ownership capability tokens.
            for state in (event['before'], event['after']):
                if isinstance(state.get('owner'), dict):
                    state['owner'].pop('token', None)
            report['resources'].append(event)
        container_item = receipt['preflight']['binaries'].get('examples/container-fixture')
        for journal in sorted(root.rglob('container.json')):
            owned = journal.parent
            if not owned.resolve().is_relative_to(root) or journal.is_symlink():
                raise Problem('incomplete', 'Symlinked container resource refused.')
            binary = REPO / 'target/debug/examples/container-fixture'
            if not container_item or sha(binary) != container_item['sha256']:
                raise Problem('incomplete', 'Container helper binary identity changed.')
            def container(op):
                return json.loads(capture([str(binary)], timeout=30, input=json.dumps(
                    {'root': str(owned), 'socket': receipt['preflight'].get('dockerSocket') or docker_socket(), 'op': op})))
            state = container('inspect')
            event = {'root': str(owned), 'before': state}
            if state.get('state') == 'running':
                report['fallback'] = True
                container('stop')
                state = container('inspect')
            retained = json.loads(journal.read_text())
            if state.get('state') == 'stopped' and retained.get('removed') is not True:
                report['fallback'] = True
                # The owned container boundary saves final output before removal.
                # Keep it in the private journal rather than copying output into
                # the sanitized run receipt.
                container('receipt')
                container('remove')
            event['after'] = container('inspect')
            retained = json.loads(journal.read_text())
            event['removed'] = retained.get('removed') is True
            event['receiptRetained'] = isinstance(retained.get('receipt'), dict)
            if event['after'].get('state') != 'stopped' or not event['removed'] or not event['receiptRetained']:
                report['remaining'].append(str(owned))
            report['resources'].append(event)
    signals, remaining = processes.stop(processes.Processes(), REPO, [], receipt['id'], 5, True)
    report['signals'] = signals
    report['fallback'] = report['fallback'] or bool(signals)
    report['remaining'].extend(processes.summary(remaining))
    report.update(passed=not report['remaining'], endedAt=time.time())
    receipt.setdefault('cleanupAttempts', []).append(report)
    if report['remaining'] or report['fallback']:
        receipt.update(outcome='incomplete', reason='Owned fallback cleanup was needed or ownership remains unresolved; retained for diagnosis.')
    write_json(path, receipt)
    return report


def finish(root, receipt, outcome, reason):
    receipt.update(outcome=outcome, reason=reason, endedAt=time.time())
    write_json(root / 'receipt.json', receipt)
    try:
        cleanup = owned_cleanup(root / 'receipt.json')
    except (Problem, OSError, ValueError, KeyError, TypeError) as error:
        cleanup = {'passed': False, 'reason': str(error), 'endedAt': time.time()}
        receipt.setdefault('cleanupAttempts', []).append(cleanup)
        receipt.update(outcome='incomplete', reason=f'Owned cleanup could not be confirmed: {error}')
        write_json(root / 'receipt.json', receipt)
    receipt = processes.private_json(root / 'receipt.json')
    receipt['artifacts'] = {str(p.relative_to(root)): sha(p) for p in sorted(root.rglob('*'))
                            if p.is_file() and not p.is_symlink() and p != root / 'receipt.json'}
    write_json(root / 'receipt.json', receipt)
    return {'outcome': receipt['outcome'], 'reason': receipt['reason'], 'receipt': str(root / 'receipt.json'),
            'cleanup': cleanup['passed']}


def build(directory):
    root, receipt = new_run(directory, 'build')
    env = env_for(receipt)
    try:
        if os.environ.get('CARGO_TARGET_DIR'):
            raise Problem('unavailable', 'Unset CARGO_TARGET_DIR for the documented source/debug build.')
        before = source_identity()
        receipt['revision'] = revision()
        code, log = run_command([sys.executable, 'scripts/bootstrap-native.py', '--check-build-dependencies'], root, receipt, env, 600, 'bootstrap')
        if code:
            raise Problem('unavailable', 'Native bootstrap/dependency check failed; inspect retained bootstrap log.')
        env['ZIG'] = log.read_text().strip().splitlines()[-1]
        commands = [(['cargo', 'build', '--workspace', '--examples', '--locked'], 'examples'),
                    (['cargo', 'build', '--workspace', '--locked'], 'workspace'),
                    (['cargo', 'test', '-p', 'crewroom-desktop', '--locked', '--no-run', '--message-format=json-render-diagnostics'], 'native-test')]
        for command, label in commands:
            code, log = run_command(command, root, receipt, env, 1800, label)
            if code:
                raise Problem('failed', f'Native {label} build failed; inspect {log}.')
        artifacts = []
        for line in log.read_text().splitlines():
            try:
                item = json.loads(line)
            except ValueError:
                continue
            if item.get('reason') == 'compiler-artifact' and item.get('profile', {}).get('test') and item.get('executable'):
                artifacts.append(item['executable'])
        if len(set(artifacts)) != 1:
            raise Problem('incomplete', f'Expected one native test executable; build reported {len(set(artifacts))}.')
        paths = {name: REPO / 'target/debug' / name for name in BINARY_NAMES}
        paths['nativeTest'] = Path(artifacts[0])
        receipt['binaries'] = {name: {'path': str(path), 'sha256': sha(path)} for name, path in paths.items()}
        receipt['source'] = source_identity()
        if before != receipt['source']:
            raise Problem('stale', 'Native source changed during build; rebuild after edits settle.')
        receipt['toolchain'] = {tool: capture([tool, '--version']) for tool in ('cargo', 'rustc', 'node')}
        receipt['zig'] = {'path': env['ZIG'], 'sha256': sha(env['ZIG'])}
        result = finish(root, receipt, 'passed', 'Locked debug binaries and native driver are bound to source contents.')
        if result['outcome'] == 'passed':
            BUILD.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            write_json(BUILD, processes.private_json(root / 'receipt.json'))
        return result
    except (Problem, OSError, ValueError, KeyError, TypeError, KeyboardInterrupt) as error:
        return finish(root, receipt, getattr(error, 'outcome', 'incomplete'), str(error) or 'Build interrupted; evidence retained.')


def run_recipe(recipe, manifest, directory):
    root, receipt = new_run(directory, 'native-feature', recipe['key'])
    try:
        # workflowFixture uses mkdtemp('cr-wf-') (six generated ASCII bytes).
        # Match RuntimeConfig::parse's Linux sockaddr_un.sun_path bound before
        # launching the fixture; filesystem paths use bytes, not Unicode length.
        socket_path = root / 'fixtures/cr-wf-XXXXXX/runtime/tmux.sock'
        socket_bytes = len(os.fsencode(socket_path))
        receipt['fixturePaths'] = {'socketTemplate': str(socket_path), 'socketBytes': socket_bytes,
                                   'socketMaximumBytes': 107}
        if socket_bytes >= 108:
            raise Problem('unavailable', f'Evidence directory makes the fixture tmux socket path too long '
                          f'({socket_bytes} bytes; Linux permits at most 107). Choose a shorter fresh '
                          'evidence path, for example /tmp/cr-steering-01; no journey was launched.')
        receipt['preflight'] = preflight(recipe, manifest)
        receipt['revision'] = revision()
        receipt['expectedTests'] = 1
        receipt['expectedAssertions'] = recipe['assertions']
        env = env_for(receipt)
        env.update(CREWROOM_NATIVE_TEST_BINARY=receipt['preflight']['binaries']['nativeTest']['path'],
                   CREWROOM_NATIVE_BINARY_DIRECTORY=str(REPO / 'target/debug'),
                   CREWROOM_FEATURE_FIXTURE_PARENT=str(root / 'fixtures'),
                   CREWROOM_FEATURE_RESOURCE_ROOT=str(root / 'resources'),
                   CREWROOM_FEATURE_RESULT=str(root / 'journey.json'), CREWROOM_RETAIN_FIXTURE='1')
        if receipt['preflight'].get('dockerSocket'):
            # The journey uses the same engine the preflight reached.
            env['CREWROOM_DOCKER_SOCKET'] = receipt['preflight']['dockerSocket']
        command = ['node', '--import', 'tsx', '--test', '--test-reporter=tap', '--test-concurrency=1',
                   '--test-name-pattern', '^' + recipe['key'] + '$', ENTRY]
        code, log = run_command(command, root, receipt, env, recipe['timeoutSeconds'], 'journey')
        counts = {name: int(value) for name, value in re.findall(r'^# (tests|pass|fail|cancelled|skipped|todo) (\d+)\s*$', log.read_text(), re.M)}
        receipt['testCounts'] = counts
        # Node can report one successful file-level test when a name pattern
        # selects no named tests. Counters alone cannot establish selection.
        names = re.findall(r'^# Subtest: (.+)$', log.read_text(), re.M)
        receipt['selectedTests'] = names
        if names != [recipe['key']] or counts.get('tests') != 1 or counts.get('pass') != 1 or any(counts.get(k, 0) for k in ('fail', 'cancelled', 'skipped', 'todo')):
            raise Problem('failed' if code else 'incomplete', f'Exactly one passing selected test is required; observed {counts}.')
        if code:
            raise Problem('failed', f'Journey process exited {code}; retained logs explain the failure.')
        if not (root / 'journey.json').is_file():
            raise Problem('incomplete', 'Journey returned no native/durable assertion evidence.')
        journey = json.loads((root / 'journey.json').read_text())
        assertions = journey.get('assertions', [])
        passed = [a['id'] for a in assertions if a.get('passed') is True]
        receipt['assertionCounts'] = {'expected': len(recipe['assertions']), 'executed': len(assertions), 'passed': len(passed)}
        if journey.get('recipe') != recipe['key'] or journey.get('outcome') != 'passed' or set(passed) != set(recipe['assertions']) or len(assertions) != len(recipe['assertions']):
            raise Problem('incomplete', 'Required native journey assertion IDs are missing, duplicated or failed.')
        if not journey.get('native') or not journey.get('durable') or journey.get('cleanup', {}).get('passed') is not True:
            raise Problem('incomplete', 'Native observations, durable snapshots or normal fixture cleanup evidence is missing.')
        if Path(journey.get('fixtureRoot', '/')).resolve().parent != root / 'fixtures':
            raise Problem('incomplete', 'Journey fixture escaped its private run directory.')
        if receipt['preflight']['manifestSha256'] != sha(manifest) or receipt['preflight']['sourceSha256'] != source_identity()['sha256'] or any(
                sha(relative_file(p)) != digest for p, digest in receipt['preflight']['recipeSources'].items()):
            raise Problem('stale', 'Source or recipe/helper content changed during execution; rerun after edits settle.')
        if any(sha(item['path']) != item['sha256'] for item in receipt['preflight']['binaries'].values()):
            raise Problem('stale', 'Executable content changed during the journey.')
        return finish(root, receipt, 'passed', 'One selected native journey, all required assertions and owned cleanup passed.')
    except (Problem, OSError, ValueError, KeyError, TypeError, KeyboardInterrupt) as error:
        return finish(root, receipt, getattr(error, 'outcome', 'incomplete'), str(error) or 'Interrupted; evidence retained.')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--manifest', type=Path, default=MANIFEST, help='Disposable manifest copy for the stale-selector drill; commands remain fixed.')
    sub = parser.add_subparsers(dest='operation', required=True)
    sub.add_parser('list')
    for operation in ('check', 'run'):
        command = sub.add_parser(operation)
        command.add_argument('key', choices=KEYS)
        if operation == 'run':
            command.add_argument('--evidence-dir', type=Path, required=True)
    command = sub.add_parser('build')
    command.add_argument('--evidence-dir', type=Path, required=True)
    command = sub.add_parser('cleanup')
    command.add_argument('--receipt', type=Path, required=True)
    args = parser.parse_args()
    # SIGTERM follows the same evidence-preserving path as Ctrl-C.
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    try:
        if args.operation == 'cleanup':
            result = owned_cleanup(args.receipt.absolute())
            report = {'outcome': 'passed' if result['passed'] else 'incomplete', 'cleanup': result,
                      'note': 'Cleanup success does not promote an interrupted journey to passed.'}
        elif args.operation == 'build':
            report = build(args.evidence_dir)
        else:
            manifest = load_manifest(args.manifest)
            if args.operation == 'list':
                report = {'outcome': 'ready', 'recipes': [{k: r[k] for k in ('key', 'title', 'aliases', 'recipePath', 'cases')} for r in manifest['recipes']]}
            else:
                recipe = next(r for r in manifest['recipes'] if r['key'] == args.key)
                report = run_recipe(recipe, args.manifest, args.evidence_dir) if args.operation == 'run' else {
                    'outcome': 'ready', 'preflight': preflight(recipe, args.manifest)}
    except (Problem, OSError, ValueError, KeyError, TypeError) as error:
        report = {'outcome': getattr(error, 'outcome', 'incomplete'), 'reason': str(error)}
    print(json.dumps(report, indent=2))
    return EXIT[report['outcome']]


if __name__ == '__main__':
    raise SystemExit(main())
