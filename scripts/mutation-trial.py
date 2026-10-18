#!/usr/bin/env python3
"""Run FOR-13's five fixed artifact mutants in a disposable private checkout."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time
import uuid

import test_processes as owned

REPO = Path(__file__).resolve().parents[1]
CONFIG = REPO / 'docs/testing/mutations/artifacts/config.json'
TARGET = 'crates/crewroom-runtime/src/evidence/artifacts.rs'
TEST = 'scripts/rust/evidence.test.ts'
NAME = 'native artifacts enforce byte hashes, identities, size and regular private files'
PREIMAGE = '283149d06fbcd73fb06f852892013d836df246c0fc8909735f7c9ef5bbebc763'
BUDGETS = {'baseline': 600, 'build': 180, 'test': 45, 'total': 1800}


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def write_json(path, value):
    with Path(path).open('x') as out:
        json.dump(value, out, indent=2)
        out.write('\n')
    Path(path).chmod(0o600)


def load_config(path=CONFIG):
    config = json.loads(path.read_text())
    if (config.get('schemaVersion') != 1 or config.get('target') != TARGET or
            config.get('preimageSha256') != PREIMAGE or config.get('testFile') != TEST or
            config.get('testName') != NAME or config.get('budgetsSeconds') != BUDGETS or
            config.get('workers') != 1 or [p['id'] for p in config['patches']] !=
            ['M1', 'M2', 'M3', 'M4', 'M5']):
        raise ValueError('Only the fixed FOR-13 artifact configuration is supported.')
    patches = []
    for item in config['patches']:
        if item['file'] != item['id'] + '.json':
            raise ValueError('Patch paths must be fixed local filenames.')
        patch_path = path.parent / item['file']
        if patch_path.is_symlink() or sha(patch_path) != item['sha256']:
            raise ValueError('Patch identity mismatch.')
        patch = json.loads(patch_path.read_text())
        if (patch['id'] != item['id'] or patch['path'] != TARGET or
                patch['preimageSha256'] != PREIMAGE or not patch['old'] or
                patch['old'] == patch['new']):
            raise ValueError('Patch target/preimage mismatch.')
        patches.append(patch)
    return config, patches


def apply_patch(checkout, patch):
    path = checkout / TARGET
    if patch['path'] != TARGET or path.is_symlink() or path.resolve() != checkout.resolve() / TARGET:
        raise ValueError('Mutation target escapes the fixed checkout path.')
    if sha(path) != patch['preimageSha256'] or patch['preimageSha256'] != PREIMAGE:
        raise ValueError('Unexpected artifact source preimage.')
    source = path.read_text()
    if source.count(patch['old']) != 1:
        raise ValueError('Mutation replacement must have exactly one match.')
    changed = source.replace(patch['old'], patch['new'])
    if hashlib.sha256(changed.encode()).hexdigest() != patch['postimageSha256']:
        raise ValueError('Mutation postimage mismatch.')
    path.write_text(changed)


def restore(checkout, source):
    path = checkout / TARGET
    path.write_bytes(source)
    if sha(path) != PREIMAGE:
        raise ValueError('Exact baseline restore failed.')


def selected_result(stdout):
    matches = re.findall(r'^(ok|not ok) \d+ - ' + re.escape(NAME) + r'$', stdout, re.M)
    tests = re.findall(r'^# tests (\d+)$', stdout, re.M)
    if len(matches) != 1 or tests != ['1'] or re.search(r'^# (cancelled|skipped|todo) [1-9]', stdout, re.M):
        raise ValueError('Expected exactly one executed selected test.')
    return {'selected': 1, 'passed': matches[0] == 'ok',
            'fixtureExitVerified': '"event":"native-fixture-exit","exitCode":0,"signal":null' in stdout}


def classify(build, test=None, stdout='', mutant=None):
    if build.get('timedOut') or (test and test.get('timedOut')):
        return 'timeout'
    if build['exitCode'] != 0:
        return 'unviable'
    if test is None:
        return 'not_run'
    try:
        selection = selected_result(stdout)
    except ValueError:
        return 'infrastructure_error'
    if not selection['fixtureExitVerified'] or test.get('remaining') or test.get('unexpectedCleanup'):
        return 'infrastructure_error'
    if test['exitCode'] == 0 and selection['passed']:
        return 'survived'
    expected = {
        'M1': 'Missing expected rejection: artifact hash mismatch must reject',
        'M2': 'Missing expected rejection: artifact byte count mismatch must reject',
        'M3': 'Missing expected rejection: artifact hash mismatch must reject',
        'M4': 'Missing expected rejection: artifact oversized write must reject',
        'M5': 'Got unwanted rejection: artifact exact-limit write must succeed',
    }
    if (test['exitCode'] != 0 and not selection['passed'] and mutant in expected and
            re.search(r"^  error: (?:'" + re.escape(expected[mutant]) + r"'|\|-\n    " + re.escape(expected[mutant]) + r")$", stdout, re.M)):
        return 'killed'
    return 'infrastructure_error'


class Trial:
    def __init__(self, directory, previous=None):
        self.directory = directory
        self.checkout = directory / 'checkout'
        self.started = time.time() if previous is None else previous['startedEpoch']
        self.deadline = self.started + BUDGETS['total']
        self.rows = []
        self.commands = []
        self.env = dict(os.environ)
        for key in ('CARGO_TARGET_DIR', 'CREWROOM_NATIVE_BINARY_DIRECTORY'):
            self.env.pop(key, None)
        self.env['CARGO_BUILD_JOBS'] = '2'
        self.env['TMPDIR'] = str(directory / 'tmp')
        Path(self.env['TMPDIR']).mkdir(mode=0o700)
        self.run_id = str(uuid.uuid4())
        write_json(owned.registry(REPO, create=True) / (self.run_id + '.json'),
                   {'id': self.run_id, 'repo': str(REPO), 'label': 'FOR-13 artifact mutation trial',
                    'created': time.time(), 'roots': [str(directory)]})
        self.env.update({owned.RUN_KEY: self.run_id, owned.REPO_KEY: str(REPO)})

    def cleanup(self):
        probe = owned.Processes()
        signals, remaining = owned.stop(probe, REPO, [], self.run_id, 2, True)
        return {'signals': signals, 'remaining': owned.summary(remaining)}

    def command(self, label, argv, timeout, cwd=None, extra_env=None, cleanup=False):
        budget = timeout if cleanup else min(timeout, self.deadline - time.time())
        if budget <= 0:
            raise TimeoutError('Trial total budget exhausted.')
        out_path = self.directory / (label + '.stdout')
        err_path = self.directory / (label + '.stderr')
        start = time.monotonic()
        row = {'label': label, 'argv': argv, 'cwd': str(cwd or self.checkout),
               'timeoutSeconds': round(budget, 3), 'timedOut': False}
        with out_path.open('xb') as out, err_path.open('xb') as err:
            child = subprocess.Popen(argv, cwd=cwd or self.checkout,
                                     env=dict(self.env, **(extra_env or {})), stdout=out, stderr=err)
            try:
                row['exitCode'] = child.wait(timeout=budget)
            except subprocess.TimeoutExpired:
                row['timedOut'] = True
                row.update(self.cleanup())
                row['exitCode'] = child.wait(timeout=5)
        row['seconds'] = round(time.monotonic() - start, 3)
        row['stdout'] = {'path': out_path.name, 'sha256': sha(out_path)}
        row['stderr'] = {'path': err_path.name, 'sha256': sha(err_path)}
        self.commands.append(row)
        return row

    def build(self, label, baseline=False, timeout=None):
        # Remove the directly launched executable so success requires this invocation
        # to materialize it in this checkout; never resolve main's executable.
        binary = self.checkout / 'target/debug/examples/repository-fixture'
        binary.unlink(missing_ok=True)
        argv = ['cargo', 'build', '-p', 'crewroom-runtime', '--example', 'repository-fixture', '--locked']
        if baseline:
            (self.checkout / 'target/debug/crewroom-process').unlink(missing_ok=True)
            argv += ['--bin', 'crewroom-process']
        result = self.command(label, argv, timeout or BUDGETS['build'])
        if result['exitCode'] == 0 and not result['timedOut']:
            result['binaries'] = {path: sha(self.checkout / path) for path in (
                'target/debug/examples/repository-fixture', 'target/debug/crewroom-process')}
        return result

    def test(self, label, node):
        retention = self.directory / (label + '-fixture')
        result = self.command(label, [node, '--import', 'tsx', '--test', '--test-reporter=tap',
            '--test-concurrency=1', '--test-name-pattern', '^' + NAME + '$', TEST], BUDGETS['test'],
            extra_env={'CREWROOM_MUTATION_RETENTION_DIR': str(retention)})
        stdout = (self.directory / result['stdout']['path']).read_text()
        try:
            result.update(selected_result(stdout))
        except ValueError as error:
            result['selectionError'] = str(error)
        # Snapshot the exact test and source alongside output before restoring/removing.
        snapshot = self.directory / (label + '-sources')
        snapshot.mkdir(mode=0o700)
        for source in (TARGET, TEST):
            shutil.copy2(self.checkout / source, snapshot / Path(source).name)
        result['sourceSha256'] = sha(self.checkout / TARGET)
        result['testSha256'] = sha(self.checkout / TEST)
        result['retainedFixture'] = retention.exists()
        cleanup = self.cleanup()
        result.update(cleanup)
        if cleanup['signals']:
            result['unexpectedCleanup'] = True
        return result, stdout


def run(directory, node, recheck_from=None):
    config, patches = load_config()
    previous = None
    if recheck_from:
        previous = owned.private_json(recheck_from)
        if (type(previous.get('startedEpoch')) not in (int, float) or
                not 0 <= time.time() - previous['startedEpoch'] < BUDGETS['total'] or
                [row.get('id') for row in previous.get('mutants', [])] != ['M1', 'M2', 'M3', 'M4', 'M5']):
            raise ValueError('Original trial identity/deadline is invalid or exhausted.')
        if (previous.get('mode') != 'initial' or not previous.get('complete') or
                previous.get('targetSha256') != PREIMAGE or previous.get('configSha256') != sha(CONFIG)):
            raise ValueError('Recheck requires a complete original fixed trial receipt.')
        survivors = [row['id'] for row in previous['mutants'] if row['outcome'] == 'survived']
        if not 1 <= len(survivors) <= 2:
            raise ValueError('Recheck permits only one or two demonstrated survivors.')
        patches = [patch for patch in patches if patch['id'] in survivors]
        if previous['testSha256'] == sha(REPO / TEST):
            raise ValueError('Recheck requires the bounded assertion improvement.')
    baseline = (REPO / TARGET).read_bytes()
    if hashlib.sha256(baseline).hexdigest() != PREIMAGE:
        raise ValueError('Main target does not match the planned source.')
    if directory.exists() or directory.is_symlink():
        raise ValueError('Evidence directory already exists; retained evidence is never overwritten.')
    directory = owned.fixture_root(str(directory), REPO)
    directory.mkdir(parents=True, mode=0o700)
    trial = Trial(directory, previous)
    shutil.copyfile(__file__, directory / 'runner.py')
    (directory / 'runner.py').chmod(0o600)
    revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=REPO, text=True).strip()
    report = {'schemaVersion': 1, 'mode': 'recheck' if previous else 'initial', 'revision': revision,
              'startedEpoch': trial.started, 'runId': trial.run_id, 'targetSha256': PREIMAGE,
              'runnerSha256': sha(__file__), 'configSha256': sha(CONFIG), 'testSha256': sha(REPO / TEST),
              'budgetsSeconds': BUDGETS, 'workers': 1, 'complete': False, 'mutants': trial.rows,
              'toolchain': {tool: subprocess.check_output([tool, '--version'], text=True).strip() for tool in (node, 'cargo', 'rustc', 'git')},
              'commands': trial.commands, 'limits': 'Local scripted artifacts fixture; no provider or Docker cases.'}
    if recheck_from:
        report['previous'] = {'path': str(recheck_from), 'sha256': sha(recheck_from)}
    created = False
    try:
        setup_start = time.monotonic()
        result = trial.command('worktree-add', ['git', 'worktree', 'add', '--detach', str(trial.checkout), revision], 60, REPO)
        if result['exitCode']:
            raise ValueError('Worktree setup failed.')
        created = True
        shutil.copy2(REPO / TEST, trial.checkout / TEST)
        (trial.checkout / 'node_modules').symlink_to(REPO / 'node_modules', target_is_directory=True)
        report['dependencies'] = {'cargoLockSha256': sha(trial.checkout / 'Cargo.lock'),
                                  'packageLockSha256': sha(trial.checkout / 'package-lock.json'),
                                  'nodeModules': 'Read-only resolution through main lockfile-installed dependencies.'}
        # COW cache copies contain dependency files only; project artifacts/fingerprints
        # are removed, followed by explicit project builds into this checkout.
        debug = trial.checkout / 'target/debug'
        debug.mkdir(parents=True)
        for name in ('deps', '.fingerprint', 'build'):
            cache = REPO / 'target/debug' / name
            if cache.exists():
                result = trial.command('cache-' + name.lstrip('.'), ['cp', '-a', '--reflink=auto', str(cache), str(debug / name)], 120, REPO)
                if result['exitCode']:
                    raise ValueError('Private dependency cache copy failed.')
                for path in (debug / name).iterdir():
                    if 'crewroom' in path.name:
                        if path.is_dir():
                            shutil.rmtree(path)
                        else:
                            path.unlink()
        remaining = BUDGETS['baseline'] - (time.monotonic() - setup_start)
        if remaining <= 0:
            raise TimeoutError('Baseline setup budget exhausted.')
        build = trial.build('baseline-build', True, remaining)
        report['baselineBuild'] = build
        if build['exitCode'] or build['timedOut']:
            raise ValueError('Baseline build did not complete.')
        test, output = trial.test('baseline-test', node)
        report['baseline'] = test
        if classify(build, test, output) != 'survived' or not test['retainedFixture']:
            raise ValueError('Unmodified selected baseline must pass with retained state and verified fixture exit.')
        for patch in patches:
            row = {'id': patch['id'], 'outcome': 'not_run', 'patchSha256': sha(CONFIG.parent / (patch['id'] + '.json'))}
            trial.rows.append(row)
            restore(trial.checkout, baseline)
            apply_patch(trial.checkout, patch)
            changed = subprocess.check_output(['git', 'diff', '--name-only'], cwd=trial.checkout, text=True).splitlines()
            if set(changed) - {TARGET, TEST}:
                raise ValueError('Unexpected changed path in isolated checkout.')
            row['sourceSha256'] = sha(trial.checkout / TARGET)
            try:
                build = trial.build(patch['id'] + '-build')
                row['build'] = build
                if build['exitCode'] or build['timedOut']:
                    row['outcome'] = classify(build)
                    if build['timedOut']:
                        break
                    continue
                test, output = trial.test(patch['id'] + '-test', node)
                row['test'] = test
                row['outcome'] = classify(build, test, output, patch['id'])
                if test.get('unexpectedCleanup'):
                    row['outcome'] = 'infrastructure_error'
                if test['timedOut'] or test.get('remaining'):
                    break
            finally:
                restore(trial.checkout, baseline)
                row['restoredSha256'] = sha(trial.checkout / TARGET)
        report['complete'] = (any(row['outcome'] in ('killed', 'survived') for row in trial.rows) and
                              all(row['outcome'] in ('killed', 'survived', 'unviable') for row in trial.rows))
    except (OSError, ValueError, TimeoutError, subprocess.SubprocessError) as error:
        report['error'] = str(error)
    finally:
        recorded = {row['id'] for row in trial.rows}
        trial.rows.extend({'id': p['id'], 'outcome': 'not_run'} for p in patches if p['id'] not in recorded)
        report['cleanup'] = trial.cleanup()
        if created and not report['cleanup']['remaining']:
            restore(trial.checkout, baseline)
            report['restoredSha256'] = sha(trial.checkout / TARGET)
            result = trial.command('worktree-remove', ['git', 'worktree', 'remove', '--force', str(trial.checkout)], 60, REPO, cleanup=True)
            report['worktreeRemoved'] = result['exitCode'] == 0 and not trial.checkout.exists()
        report['mainSourceUnchanged'] = sha(REPO / TARGET) == PREIMAGE
        report['secondsSinceTrialStart'] = round(time.time() - trial.started, 3)
        report['complete'] = bool(report['complete'] and not report['cleanup']['remaining'] and
                                  report.get('worktreeRemoved') and report['mainSourceUnchanged'])
        write_json(directory / 'receipt.json', report)
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('module', choices=['artifacts'])
    parser.add_argument('--evidence-dir', type=Path, required=True)
    parser.add_argument('--node', default='node', help='Node >=24 executable; select documented Node 26 on this host.')
    parser.add_argument('--recheck-from', type=Path, help='Complete original receipt; rerun its at most two survivors after improved assertions.')
    args = parser.parse_args()
    os.umask(0o077)
    try:
        if sys.version_info < (3, 12):
            raise ValueError('Python >=3.12 required.')
        node = shutil.which(args.node)
        if not node or int(subprocess.check_output([node, '--version'], text=True).strip().lstrip('v').split('.')[0]) < 24:
            raise ValueError('Node >=24 required.')
        if not (REPO / 'node_modules/tsx').exists():
            raise ValueError('Install locked Node dependencies before the trial.')
        report = run(args.evidence_dir.absolute(), node, args.recheck_from)
        print(json.dumps({'receipt': str(args.evidence_dir / 'receipt.json'), 'complete': report['complete'],
                          'outcomes': {row['id']: row['outcome'] for row in report['mutants']}, 'error': report.get('error')}))
        return 0 if report['complete'] else 2
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print('Mutation trial refused: ' + str(error), file=sys.stderr)
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
