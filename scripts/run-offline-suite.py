#!/usr/bin/env python3
"""Execute the current release manifest without upgrading partial evidence to a pass.

Use --only for a bounded diagnostic run. Exit zero requires every mapped case and
scenario to have complete coverage and passing, same-source command receipts.
The retired runner and historical Relaycast experiments are never launched here.
"""
import argparse
from collections import Counter
import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import shutil
import subprocess
import sys
import uuid

PROJECT = Path(__file__).resolve().parent.parent
MANIFEST = PROJECT / 'docs/testing/release-manifest.json'


def runtime_identity():
    selected = {'node': ['node', '--version'], 'npm': ['npm', '--version'], 'python': [sys.executable, '--version'],
                'git': ['git', '--version'], 'tmux': ['tmux', '-V'], 'docker': ['docker', 'version', '--format', '{{.Server.Version}}'],
                'omp': [str(Path.home() / '.bun/bin/omp'), '--version'],
                'rustMail': [str(PROJECT / '.crewroom/experiments/rust-mail-prep/runtime/mcp-agent-mail'), '--version']}
    identities = {}
    def fingerprint(path):
        binary = Path(path).resolve(strict=True)
        with binary.open('rb') as handle:
            return {'path': str(binary), 'sha256': hashlib.file_digest(handle, 'sha256').hexdigest()}
    for name, command in selected.items():
        binary = Path(shutil.which(command[0]) or command[0]).resolve(strict=True)
        result = subprocess.run(command, cwd=PROJECT, capture_output=True, text=True, timeout=15, check=True)
        identities[name] = {**fingerprint(binary),
                            'version': result.stdout.strip() or result.stderr.strip()}
    version = identities['omp']['version'].removeprefix('omp/')
    addon = Path.home() / '.omp/natives' / version / 'pi_natives.darwin-arm64.node'
    identities['ompNative'] = fingerprint(addon)
    identities['processInspector'] = fingerprint(os.environ.get('CREWROOM_PROCESS_INSPECTOR', '/bin/ps'))
    identities['systemProcessInspector'] = fingerprint('/bin/ps')
    return identities


def source_identity():
    names = subprocess.check_output(['git', 'ls-files', '--cached', '--others', '--exclude-standard'], cwd=PROJECT, text=True).splitlines()
    selected = sorted(set(name for name in names if name.startswith(('server/', 'shared/', 'src/', 'tests/', 'scripts/', 'docs/testing/fixtures/')) or name in
                          ['package.json', 'package-lock.json', 'tsconfig.json', 'vite.config.ts', 'playwright.config.ts', 'index.html', 'docs/testing/release-manifest.json', 'docs/testing/suite.json', 'docs/testing/scenarios.json', 'docs/testing/model-profiles.json']))
    hashes = {name: hashlib.sha256((PROJECT / name).read_bytes()).hexdigest() for name in selected if (PROJECT / name).is_file()}
    runtime = runtime_identity()
    return {'runtime': runtime, 'head': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=PROJECT, text=True).strip(),
            'sha256': hashlib.sha256(json.dumps({'files': hashes, 'runtime': runtime}, sort_keys=True).encode()).hexdigest(), 'files': hashes}


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(',', ':')).encode()).hexdigest()


def receipt_identity(manifest, job, source):
    # A source hash alone cannot validate changed commands, coverage decisions or fixtures.
    return digest({'source': source, 'manifest': manifest, 'job': job})


def receipt_status(manifest, receipts, job, source):
    receipt = receipts.get(job, {})
    if receipt.get('identity') != receipt_identity(manifest, job, source):
        return 'not-run-on-this-candidate'
    return receipt.get('status', 'not-run')


def save_candidate(root, source, manifest):
    candidate = {'source': source, 'manifest': manifest}
    path = root / ('candidate-' + digest(candidate) + '.json')
    if not path.exists():
        with path.open('x') as handle:
            handle.write(json.dumps(candidate, indent=2) + '\n')
    return path.name


def validate(manifest):
    suite = json.loads((PROJECT / 'docs/testing/suite.json').read_text())
    scenarios = json.loads((PROJECT / 'docs/testing/scenarios.json').read_text())
    for key, specified in [('cases', suite['cases']), ('scenarios', scenarios['scenarios'])]:
        ids = [item['id'] for item in manifest[key]]
        if len(ids) != len(set(ids)) or set(ids) != {item['id'] for item in specified}:
            raise ValueError(f'{key}: missing, unknown or duplicate IDs')
        for item in manifest[key]:
            if item['coverage'] not in ['mapped', 'partial', 'deferred'] or not item.get('note') or not item.get('jobs'):
                raise ValueError(f"{item['id']}: missing coverage decision")
            if any(job not in manifest['jobs'] for job in item['jobs']):
                raise ValueError(f"{item['id']}: unknown execution job")
            for path in item.get('tests', []):
                if not (PROJECT / path).is_file(): raise ValueError(f"{item['id']}: missing test {path}")
    return manifest


def summarize(manifest, receipts, source):
    result = {}
    for group in ['cases', 'scenarios']:
        rows = []
        for item in manifest[group]:
            statuses = {job: receipt_status(manifest, receipts, job, source) for job in item['jobs']}
            status = 'passed' if item['coverage'] == 'mapped' and all(value == 'passed' for value in statuses.values()) else (
                'coverage-gap' if item['coverage'] != 'mapped' else 'failed' if 'failed' in statuses.values() else 'not-run')
            rows.append({**item, 'status': status, 'execution': statuses})
        result[group] = rows
        result[group + 'Counts'] = dict(Counter(row['status'] for row in rows))
    result['passed'] = all(row['status'] == 'passed' for group in ['cases', 'scenarios'] for row in result[group]) and all(
        receipt_status(manifest, receipts, name, source) == 'passed' for name in manifest['jobs'])
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--only', help='Comma-separated manifest job names; omitted jobs are never presumed passed.')
    parser.add_argument('--check-manifest', action='store_true')
    args = parser.parse_args()
    manifest = validate(json.loads(MANIFEST.read_text()))
    if args.check_manifest:
        print(json.dumps({'cases': len(manifest['cases']), 'scenarios': len(manifest['scenarios']), 'coverage': dict(Counter(c['coverage'] for c in manifest['cases']))}))
        return 0
    names = args.only.split(',') if args.only else list(manifest['jobs'])
    if len(set(names)) != len(names) or any(name not in manifest['jobs'] for name in names): raise ValueError('Choose distinct known manifest jobs.')
    # Nested matrix variants also need a tmux socket below macOS's 104-byte path limit.
    root = (args.output or PROJECT / '.crewroom' / f'r7-{uuid.uuid4().hex[:6]}').resolve()
    root.mkdir(parents=True, exist_ok=True, mode=0o700)
    report_path = root / 'report.json'
    source = source_identity()
    old = json.loads(report_path.read_text()) if report_path.exists() else {}
    receipts = old.get('commands', {})
    env = dict(os.environ, GIT_EDITOR='true', GIT_SEQUENCE_EDITOR='true', GIT_TERMINAL_PROMPT='0')
    # Respect a caller-selected supported runtime; do not silently pick another Node installation.
    major = int(subprocess.check_output(['node', '-p', 'process.versions.node.split(".")[0]'], env=env, text=True))
    if major < 24: raise ValueError('Crewroom release checks require Node >=24; select the supported Node PATH.')
    candidate = save_candidate(root, source, manifest)
    def save():
        report = {'source': source['sha256'], 'head': source['head'], 'scope': manifest['scope'],
                  'candidate': candidate, 'manifestSha256': digest(manifest), 'commands': receipts,
                  **summarize(manifest, receipts, source['sha256'])}
        report_path.write_text(json.dumps(report, indent=2) + '\n')
        return report
    for name in names:
        job = manifest['jobs'][name]
        output = root / f'p{list(manifest["jobs"]).index(name):02d}-{uuid.uuid4().hex[:4]}'
        command = [part.replace('{output}', str(output)) for part in job['command']]
        log = root / (output.name + '.log')
        receipt = {'command': command, 'log': log.name, 'source': source['sha256'], 'head': source['head'],
                   'candidate': candidate, 'identity': receipt_identity(manifest, name, source['sha256']), 'status': 'running',
                   'startedAt': datetime.datetime.now(datetime.timezone.utc).isoformat()}
        receipts[name] = receipt; save()
        print(json.dumps({'running': name, 'log': str(log)}), flush=True)
        with log.open('w') as handle:
            process = subprocess.Popen(command, cwd=PROJECT, env=env, stdout=handle, stderr=subprocess.STDOUT)
            try:
                process.wait(timeout=job['timeoutSeconds'])
                receipt['exitCode'] = process.returncode
            except (subprocess.TimeoutExpired, KeyboardInterrupt) as error:
                process.send_signal(signal.SIGINT)
                try: process.wait(timeout=20)
                except subprocess.TimeoutExpired: process.kill(); process.wait()
                receipt.update(exitCode=process.returncode, error=type(error).__name__ + ': inspect owned resources before retry')
        receipt['status'] = 'passed' if receipt['exitCode'] == 0 and 'error' not in receipt else 'failed'
        text = log.read_text()
        if re.search(r'(?:skipped|todo)\s+[1-9]\d*|[1-9]\d*\s+skipped', text):
            receipt.update(status='failed', error='Skipped or todo cases cannot satisfy a release gate.')
        if job.get('report'):
            path = output / job['report']; receipt['report'] = str(path.relative_to(root))
            if not path.exists() or json.loads(path.read_text()).get('passed') is not True:
                receipt.update(status='failed', error='Missing or nonpassing process report.')
        if source_identity()['sha256'] != source['sha256']:
            receipt.update(status='failed', error='Source changed during verification; freeze a new candidate before continuing.')
        receipt['finishedAt'] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        save(); print(json.dumps({'job': name, 'status': receipt['status'], 'error': receipt.get('error')}), flush=True)
        if 'Source changed' in receipt.get('error', '') or receipt.get('error', '').startswith('KeyboardInterrupt'): break
    report = save()
    print(json.dumps({'report': str(report_path), 'passed': report['passed'], 'cases': report['casesCounts'], 'scenarios': report['scenariosCounts']}), flush=True)
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    sys.exit(main())
