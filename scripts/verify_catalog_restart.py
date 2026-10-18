#!/usr/bin/env python3
"""Synthetic metadata crash/restart regression against the built Rust daemon."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import time
import urllib.request


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(mode=0o700, parents=True, exist_ok=False)
    binaries = Path(__file__).resolve().parent.parent / 'target/debug'
    marker = root / 'metadata-pid'
    fake = root / 'omp.py'
    fake.write_text('#!/usr/bin/python3\nimport os,sys,time,pathlib\n'
                    'if "--version" in sys.argv:\n print("omp/18.4.12");sys.exit(0)\n'
                    f'pathlib.Path({str(marker)!r}).write_text(str(os.getpid()))\n'
                    'time.sleep(180)\n')
    fake.chmod(0o700)
    config = {'root': str(root / 'runtime'), 'socket': str(root / 'runtime/tmux.sock'),
              'ompBinary': str(fake), 'catalog': [], 'policy': {}, 'models': {'providers': {}},
              'mail': {'atcDisabled': True, 'retentionSeconds': 600, 'projectRoot': str(root / 'mail'),
                       'url': 'http://127.0.0.1:9/mcp/', 'token': 'synthetic-unused'}}
    config_file = root / 'config.json'
    config_file.write_text(json.dumps(config))
    config_file.chmod(0o600)
    connection = root / 'data/native-connection.json'
    env = {'HOME': str(root), 'PATH': '/usr/bin:/bin', 'CREWROOM_RUNTIME_CONFIG': str(config_file),
           'CREWROOM_TMUX': '/usr/bin/tmux'}
    processes = []
    report = {'passed': False, 'scope': 'synthetic metadata only; no inference or assignments'}
    log = (root / 'daemon.log').open('wb')

    def start():
        connection.unlink(missing_ok=True)
        process = subprocess.Popen([str(binaries / 'crewroomd'), '--data-dir', str(root / 'data'),
                                    '--roles', str(root / 'roles.json'), '--port', '0'],
                                   env=env, stdout=log, stderr=log)
        processes.append(process)
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            if connection.exists():
                return process, json.loads(connection.read_text())
            if process.poll() is not None:
                raise RuntimeError('Disposable daemon failed to initialize')
            time.sleep(.05)
        raise TimeoutError('Disposable connection timeout')

    def request(info, path, post=False):
        req = urllib.request.Request(info['origin'] + '/api/' + path, data=b'{}' if post else None,
                                     headers={'Authorization': 'Bearer ' + info['token'],
                                              'Origin': info['origin'], 'Content-Type': 'application/json'})
        with urllib.request.urlopen(req, timeout=5) as response:
            return json.load(response)

    def owned(operation):
        return [json.loads(subprocess.check_output([str(binaries / 'crewroom-process'), operation,
                                                   str(path)], timeout=15))
                for path in (root / 'runtime/model-catalog/processes').glob('*/*/*') if path.is_dir()]

    try:
        first, info = start()
        request(info, 'model-catalog/refresh', True)
        deadline = time.monotonic() + 10
        while not marker.exists() and time.monotonic() < deadline:
            time.sleep(.05)
        assert marker.exists(), 'Metadata exchange did not start'
        first.kill()
        first.wait(timeout=10)
        _, info = start()
        catalog = request(info, 'model-catalog')
        assert catalog['harnesses'][0]['outcome'] == 'interrupted'
        assert all(item['state'] == 'stopped' for item in owned('inspect'))
        assert not request(info, 'workspace')['runs']
        report.update(passed=True, outcome='interrupted', ownedProcessesStoppedBeforeReady=True,
                      assignmentsCreated=0)
    except Exception as error:
        report['error'] = str(error)
        raise
    finally:
        for process in processes:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
                    report.update(passed=False, cleanupError='Daemon required forced termination')
        report['cleanupStopped'] = all(item['state'] == 'stopped' for item in owned('stop'))
        report['passed'] = report['passed'] and report['cleanupStopped']
        log.close()
        (root / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps(report))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
