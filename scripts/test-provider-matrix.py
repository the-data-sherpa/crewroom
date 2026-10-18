#!/usr/bin/env python3
"""Independent OMP failure fixtures; every child report must pass for this matrix to pass."""
import argparse
import concurrent.futures
import json
import os
import signal
import threading
from pathlib import Path
import subprocess
import sys
from mailbox_contract_fixture import PROJECT

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--output', type=Path, required=True)
root = parser.parse_args().output.resolve(); root.mkdir(parents=True, exist_ok=False, mode=0o700)
stopping = threading.Event()
active = set()
lock = threading.Lock()
def interrupt(signum, frame):
    stopping.set()
    with lock:
        for child in active:
            if child.poll() is None:
                try: os.killpg(child.pid, signal.SIGINT)
                except ProcessLookupError: pass
signal.signal(signal.SIGINT, interrupt)
signal.signal(signal.SIGTERM, interrupt)
faults = ['fallback', 'no-fallback', 'unavailable-fallback', 'exhausted', '401', '403', '429', 'timeout', 'disconnect', 'malformed', 'empty']

def run(fault):
    target = root / fault
    if stopping.is_set(): return {'fault': fault, 'passed': False, 'error': 'Interrupted before launch'}
    with (root / (fault + '.log')).open('w') as log:
        with lock:
            if stopping.is_set(): return {'fault': fault, 'passed': False, 'error': 'Interrupted before launch'}
            result = subprocess.Popen([sys.executable, 'scripts/test-provider-recovery.py', '--fault', fault, '--output', str(target)], cwd=PROJECT, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            active.add(result)
        try:
            result.wait(timeout=150)
        except subprocess.TimeoutExpired:
            os.killpg(result.pid, signal.SIGINT)
            try: result.wait(timeout=15)
            except subprocess.TimeoutExpired:
                os.killpg(result.pid, signal.SIGKILL); result.wait()
        finally:
            with lock: active.discard(result)
    path = target / 'report.json'
    report = json.loads(path.read_text()) if path.exists() else {}
    return {'fault': fault, 'exitCode': result.returncode, 'passed': result.returncode == 0 and report.get('passed') is True,
            'providerRequests': report.get('providerRequests'), 'report': str(path.relative_to(root))}

with concurrent.futures.ThreadPoolExecutor(max_workers=3) as pool:
    rows = list(pool.map(run, faults))
report = {'passed': not stopping.is_set() and all(row['passed'] for row in rows), 'scope': 'Real OMP processes and local workflow; scripted provider failures.', 'cases': rows}
(root / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps(report))
raise SystemExit(0 if report['passed'] else 1)
