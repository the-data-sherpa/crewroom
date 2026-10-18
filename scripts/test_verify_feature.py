"""Public CLI guards using disposable toolchain/runner fixtures, never native proof."""
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest

import test_processes

SCRIPT = Path(__file__).with_name('verify-feature.py')
KEYS = ('steering-reconnect', 'pause-resume', 'readiness-invalidation')
SELECTOR = 'workbench::journey_tests::feature_recipes::native_feature_recipe_control'


class FeatureGuardTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='crewroom-feature-guard-')
        self.repo = Path(self.temporary.name).resolve()
        self.children = []
        for name in ('scripts/rust', 'docs/testing/feature-map', 'crates/guard', 'bin'):
            (self.repo / name).mkdir(parents=True)
        shutil.copyfile(SCRIPT, self.repo / 'scripts/verify-feature.py')
        shutil.copyfile(test_processes.__file__, self.repo / 'scripts/test_processes.py')
        shutil.copyfile(SCRIPT.with_name('docker_engine.py'), self.repo / 'scripts/docker_engine.py')
        (self.repo / 'Cargo.toml').write_text('[workspace]\n')
        (self.repo / 'Cargo.lock').write_text('guard lock\n')
        (self.repo / 'crates/guard/lib.rs').write_text('// original native source\n')
        (self.repo / 'docs/testing/suite.json').write_text('{"cases":[{"id":"UI-02"}]}')
        (self.repo / 'docs/testing/scenarios.json').write_text('{"scenarios":[{"id":"E2E-05"}]}')
        (self.repo / 'scripts/rust/feature-recipes.test.ts').write_text('\n'.join(f"test('{key}', () => {{}});" for key in KEYS))
        manifest = {'schemaVersion': 1, 'recipes': []}
        for key in KEYS:
            recipe = f'docs/testing/feature-map/{key}.md'
            (self.repo / recipe).write_text(f'# {key}\nGuard fixture only.\n')
            manifest['recipes'].append(dict(key=key, title=key, aliases=[], recipePath=recipe, cases=['UI-02', 'E2E-05'],
                entryPoint='scripts/rust/feature-recipes.test.ts', testName=key, nativeSelector=SELECTOR,
                sourcePaths=['scripts/rust/feature-recipes.test.ts'], assertions=['guard-observed'],
                requiresDocker=False, timeoutSeconds=30))
        self.manifest = self.repo / 'docs/testing/feature-map/manifest.json'
        self.manifest.write_text(json.dumps(manifest))
        self.executable('tmux', "print('tmux guard')")
        self.executable('rustc', "print('rustc guard')")
        self.executable('cargo', f'''
import json, pathlib, sys
repo = pathlib.Path.cwd()
if '--version' in sys.argv:
    print('cargo guard'); raise SystemExit()
if '--no-run' in sys.argv:
    print(json.dumps({{'reason':'compiler-artifact','profile':{{'test':True}},'executable':str(repo/'target/debug/deps/native-guard')}}))
''')
        (self.repo / 'scripts/bootstrap-native.py').write_text("print('/bin/true')\n")
        for name in ('crewroomd', 'crewroom-host', 'crewroom-process', 'crewroom',
                     'examples/workflow-fixture', 'examples/container-fixture'):
            self.write_executable(self.repo / 'target/debug' / name, "print('{\"state\":\"stopped\"}')")
        self.write_executable(self.repo / 'target/debug/examples/container-fixture', '''
import json, pathlib, sys
request = json.load(sys.stdin)
root = pathlib.Path(request['root']); journal = root/'container.json'
value = json.loads(journal.read_text()); op = request['op']
with (root/'operations.log').open('a') as log: log.write(op+'\\n')
if op == 'stop': value['state'] = 'stopped'
if op == 'receipt': value['receipt'] = {'exitCode':0, 'output':'retained guard output'}
if op == 'remove':
    assert value['state'] == 'stopped' and value.get('receipt')
    value['removed'] = True
journal.write_text(json.dumps(value))
print(json.dumps(value['receipt'] if op == 'receipt' else {'state':value['state'],'id':'guard-container'}))
''')
        self.write_executable(self.repo / 'target/debug/deps/native-guard', f'''
import sys
if {SELECTOR!r} in sys.argv: print({(SELECTOR + ': test')!r})
print('1 test, 0 benchmarks')
''')
        self.executable('node', '''
import json, os, pathlib, subprocess, sys, time
if '--version' in sys.argv:
    print(os.environ.get('GUARD_NODE_VERSION', 'v24.0.0')); raise SystemExit()
if '-e' in sys.argv:
    print('tsx available'); raise SystemExit()
root = pathlib.Path(os.environ['CREWROOM_FEATURE_RESULT']).parent
mode = os.environ.get('GUARD_MODE', 'pass')
if mode == 'interrupt':
    child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(120)'], start_new_session=True,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    (root/'orphan.pid').write_text(str(child.pid))
    time.sleep(120)
key = sys.argv[sys.argv.index('--test-name-pattern') + 1].strip('^$')
if mode != 'zero':
    fixture = root/'fixtures'/'guard-fixture'; fixture.mkdir()
    (root/'journey.json').write_text(json.dumps({'recipe':key, 'outcome':'passed', 'assertions':[{'id':'guard-observed','passed':True}],
        'native':[{'surface':'guard fixture, not native acceptance'}], 'durable':[{'guard':True}],
        'fixtureRoot':str(fixture), 'cleanup':{'passed':True}}))
count = 0 if mode == 'zero' else 1
print('TAP version 13')
if count:
    print('# Subtest: '+('scripts/rust/feature-recipes.test.ts' if mode == 'file-only' else key))
    print('ok 1 - '+key)
print('1..'+str(count))
for name, value in [('tests',count),('pass',count),('fail',0),('cancelled',0),('skipped',0),('todo',0)]:
    print('# '+name+' '+str(value))
''')
        self.env = dict(os.environ, PATH=str(self.repo / 'bin') + ':' + os.environ['PATH'],
                        CREWROOM_TMUX=str(self.repo / 'bin/tmux'))
        self.env.pop('CARGO_TARGET_DIR', None)
        subprocess.run(['git', 'init', '-q', str(self.repo)], check=True)
        subprocess.run(['git', '-C', str(self.repo), 'add', '.'], check=True)
        subprocess.run(['git', '-C', str(self.repo), '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                        'commit', '-qm', 'Guard fixture'], check=True)
        result, body = self.cli('build', '--evidence-dir', str(self.repo / 'build-evidence'))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(body['outcome'], 'passed')

    def tearDown(self):
        for child in self.children:
            if child.poll() is None:
                child.kill()
            child.communicate(timeout=5)
        # Failures in the interruption guard must not leak its synthetic child.
        for pidfile in self.repo.rglob('orphan.pid'):
            pid = int(pidfile.read_text())
            proc = test_processes.Processes().read(pid)
            if proc and proc['env'].get(test_processes.REPO_KEY) == str(self.repo):
                os.kill(pid, signal.SIGKILL)
        self.temporary.cleanup()

    def write_executable(self, path, body):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text('#!' + sys.executable + '\n' + body + '\n')
        path.chmod(0o700)

    def executable(self, name, body):
        self.write_executable(self.repo / 'bin' / name, body)

    def cli(self, *args, env=None):
        result = subprocess.run([sys.executable, str(self.repo / 'scripts/verify-feature.py'), *args],
                                cwd=self.repo, env=env or self.env, text=True, capture_output=True, timeout=25)
        self.assertTrue(result.stdout, result.stderr)
        return result, json.loads(result.stdout)

    def test_missing_dependency_is_unavailable_without_launch_and_keeps_private_evidence(self):
        root = self.repo / 'unavailable'
        result, body = self.cli('run', KEYS[0], '--evidence-dir', str(root),
                                env=dict(self.env, GUARD_NODE_VERSION='v22.0.0'))
        self.assertEqual(result.returncode, 2)
        self.assertEqual(body['outcome'], 'unavailable')
        receipt = json.loads((root / 'receipt.json').read_text())
        self.assertEqual(receipt['commands'], [])
        self.assertEqual(root.stat().st_mode & 0o777, 0o700)
        self.assertEqual((root / 'receipt.json').stat().st_mode & 0o777, 0o600)
        self.assertTrue(receipt['cleanupAttempts'][-1]['passed'])

    def test_socket_path_limit_uses_bytes_and_rejects_before_any_workload(self):
        for name in ('long-evidence-directory-' * 4, 'é' * 30):
            with self.subTest(name=name):
                root = self.repo / name
                result, body = self.cli('run', KEYS[0], '--evidence-dir', str(root))
                self.assertEqual(result.returncode, 2)
                self.assertEqual(body['outcome'], 'unavailable')
                self.assertIn('shorter fresh evidence path', body['reason'])
                receipt = json.loads((root / 'receipt.json').read_text())
                self.assertEqual(receipt['commands'], [])
                self.assertNotIn('preflight', receipt)
                self.assertEqual(receipt['fixturePaths']['socketBytes'],
                                 len(os.fsencode(root / 'fixtures/cr-wf-XXXXXX/runtime/tmux.sock')))
                self.assertGreaterEqual(receipt['fixturePaths']['socketBytes'], 108)
                self.assertTrue(receipt['cleanupAttempts'][-1]['passed'])
                self.assertEqual(list((root / 'fixtures').iterdir()), [])

    def test_stale_selector_and_changed_source_or_executable_never_pass(self):
        stale = json.loads(self.manifest.read_text())
        stale['recipes'][0]['nativeSelector'] = 'nonexistent_native_test'
        copy = self.repo / 'stale-manifest.json'
        copy.write_text(json.dumps(stale))
        _, body = self.cli('--manifest', str(copy), 'check', KEYS[0])
        self.assertEqual(body['outcome'], 'stale')
        self.assertIn('Native selector', body['reason'])
        stale['recipes'][0]['nativeSelector'] = 'workbench::journey_tests::operator::native_external_operator_control'
        copy.write_text(json.dumps(stale))
        _, body = self.cli('--manifest', str(copy), 'check', KEYS[0])
        self.assertEqual(body['outcome'], 'stale')
        source = self.repo / 'crates/guard/lib.rs'
        original = source.read_text()
        source.write_text(original + '// changed\n')
        _, body = self.cli('check', KEYS[0])
        self.assertEqual(body['outcome'], 'stale')
        source.write_text(original)
        with (self.repo / 'target/debug/crewroomd').open('a') as out:
            out.write('# changed executable\n')
        _, body = self.cli('check', KEYS[0])
        self.assertEqual(body['outcome'], 'stale')
        self.assertIn('Executable content', body['reason'])

    def test_zero_selected_is_incomplete_and_retains_log(self):
        root = self.repo / 'zero'
        _, body = self.cli('run', KEYS[0], '--evidence-dir', str(root), env=dict(self.env, GUARD_MODE='zero'))
        self.assertEqual(body['outcome'], 'incomplete')
        receipt = json.loads((root / 'receipt.json').read_text())
        self.assertEqual(receipt['testCounts']['tests'], 0)
        self.assertTrue((root / '00-journey.log').is_file())
        self.assertIn('00-journey.log', receipt['artifacts'])
        _, body = self.cli('run', KEYS[0], '--evidence-dir', str(self.repo / 'file-only'),
                           env=dict(self.env, GUARD_MODE='file-only'))
        self.assertEqual(body['outcome'], 'incomplete')

    def test_guard_runner_result_requires_one_test_and_all_evidence_and_fresh_directory(self):
        root = self.repo / 'pass'
        _, body = self.cli('run', KEYS[0], '--evidence-dir', str(root))
        self.assertEqual(body['outcome'], 'passed')
        receipt = json.loads((root / 'receipt.json').read_text())
        self.assertEqual(receipt['assertionCounts'], {'expected': 1, 'executed': 1, 'passed': 1})
        before = (root / 'receipt.json').read_bytes()
        _, body = self.cli('run', KEYS[0], '--evidence-dir', str(root))
        self.assertEqual(body['outcome'], 'unavailable')
        self.assertEqual((root / 'receipt.json').read_bytes(), before)

    def test_interruption_cleans_only_registered_descendants_and_retains_failure(self):
        root = self.repo / 'interrupted'
        unrelated = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(120)'])
        self.children.append(unrelated)
        child = subprocess.Popen([sys.executable, str(self.repo / 'scripts/verify-feature.py'), 'run', KEYS[0],
                                  '--evidence-dir', str(root)], cwd=self.repo,
                                 env=dict(self.env, GUARD_MODE='interrupt'), text=True,
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.children.append(child)
        deadline = time.monotonic() + 10
        while not (root / 'orphan.pid').exists() and time.monotonic() < deadline:
            time.sleep(.03)
        self.assertTrue((root / 'orphan.pid').exists())
        pid = int((root / 'orphan.pid').read_text())
        child.terminate()
        stdout, stderr = child.communicate(timeout=15)
        self.assertEqual(json.loads(stdout)['outcome'], 'incomplete', stderr)
        self.assertIsNone(test_processes.Processes().read(pid))
        self.assertIsNone(unrelated.poll())
        receipt = json.loads((root / 'receipt.json').read_text())
        self.assertTrue(receipt['cleanupAttempts'][-1]['passed'])
        self.assertTrue(receipt['cleanupAttempts'][-1]['signals'])
        self.assertTrue((root / '00-journey.log').exists())
        _, cleaned = self.cli('cleanup', '--receipt', str(root / 'receipt.json'))
        self.assertEqual(cleaned['outcome'], 'passed')
        self.assertEqual(json.loads((root / 'receipt.json').read_text())['outcome'], 'incomplete')

    def test_foreign_or_moved_receipt_is_refused_without_signalling(self):
        protected = self.repo / '.crewroom/rust/forbidden'
        _, rejected = self.cli('run', KEYS[0], '--evidence-dir', str(protected))
        self.assertEqual(rejected['outcome'], 'incomplete')
        self.assertFalse(protected.exists())
        root = self.repo / 'completed'
        self.cli('run', KEYS[0], '--evidence-dir', str(root))
        path = root / 'receipt.json'
        receipt = json.loads(path.read_text())
        receipt['repo'] = '/foreign/checkout'
        path.write_text(json.dumps(receipt))
        _, body = self.cli('cleanup', '--receipt', str(path))
        self.assertEqual(body['outcome'], 'incomplete')
        self.assertIn('Foreign', body['reason'])
        receipt['repo'] = str(self.repo)
        receipt['id'] = '00000000-0000-4000-8000-000000000000'
        path.write_text(json.dumps(receipt))
        _, body = self.cli('cleanup', '--receipt', str(path))
        self.assertEqual(body['outcome'], 'incomplete')
        self.assertIn('not linked', body['reason'])

    def test_owned_container_running_and_stopped_retained_are_removed_with_evidence(self):
        for state in ('running', 'stopped'):
            with self.subTest(state=state):
                root = self.repo / ('container-' + state)
                _, body = self.cli('run', KEYS[0], '--evidence-dir', str(root))
                self.assertEqual(body['outcome'], 'passed')
                container = root / 'fixtures/guard-fixture/container'
                container.mkdir()
                journal = container / 'container.json'
                journal.write_text(json.dumps({'state': state}))
                _, body = self.cli('cleanup', '--receipt', str(root / 'receipt.json'))
                self.assertEqual(body['outcome'], 'passed')
                retained = json.loads(journal.read_text())
                self.assertTrue(retained['removed'])
                self.assertEqual(retained['receipt']['output'], 'retained guard output')
                operations = (container / 'operations.log').read_text().splitlines()
                self.assertEqual(operations, ['inspect'] + (['stop', 'inspect'] if state == 'running' else [])
                                 + ['receipt', 'remove', 'inspect'])
                # Recovered cleanup never turns a previously interrupted or
                # unexpectedly uncleared native journey into acceptance.
                self.assertEqual(json.loads((root / 'receipt.json').read_text())['outcome'], 'incomplete')


if __name__ == '__main__':
    unittest.main()
