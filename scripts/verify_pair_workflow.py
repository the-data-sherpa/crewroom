#!/usr/bin/env python3
"""Opt-in MH6 live pair, disposable production daemon and external Rust Mail."""
import argparse
import json
import hashlib
import os
from pathlib import Path
import shutil
import stat
import subprocess
from mailbox_contract_fixture import RustMail, free_port
from native_workflow_fixture import MailGuard


def private_config(path):
    path = path.resolve()
    info = path.stat()
    if not stat.S_ISREG(info.st_mode) or info.st_mode & 0o077 or info.st_uid != os.geteuid():
        raise ValueError('Harness configuration must be a private owned file')
    return json.loads(path.read_text())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('codex', 'claude', 'omp', 'output', 'mail'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--job', choices=('backend', 'ui'), required=True)
    parser.add_argument('--check-image', required=True)
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(mode=0o700, parents=True, exist_ok=False)
    binaries = root / 'bin'
    binaries.mkdir()
    fingerprints = {}
    for name in ('crewroomd', 'crewroom-host', 'crewroom-process'):
        source = Path('target/debug') / name
        shutil.copy2(source, binaries / name)
        fingerprints[name] = hashlib.sha256((binaries / name).read_bytes()).hexdigest()
    native_driver = Path(os.environ['CREWROOM_NATIVE_TEST_BINARY']).resolve()
    shutil.copy2(native_driver, binaries / 'native-driver')
    fingerprints['native-driver'] = hashlib.sha256((binaries / 'native-driver').read_bytes()).hexdigest()
    (root / 'binaries.json').write_text(json.dumps(fingerprints, indent=2) + '\n')
    config = {name: private_config(getattr(args, name)) for name in ('codex', 'claude', 'omp')}
    # MH6's accepted pair requires Opus. Sonnet fixture credentials may be reused;
    # the selected alias and observed model are explicit, never a fallback.
    config['claude'].update(model='opus', resolvedModel='claude-opus-5-5')
    mail = RustMail(root / 'mail', MailGuard(root, args.mail.resolve()), free_port(), binary=args.mail)
    try:
        mail.start()
        config.update(root=str(root), job=args.job, checkImage=args.check_image,
                      mail={'url': f'http://127.0.0.1:{mail.port}/mcp/', 'token': mail.token,
                            'projectRoot': mail.project, 'retentionSeconds': 86400, 'atcDisabled': True})
        settings = root / 'settings.json'
        with open(settings, 'x', opener=lambda p, f: os.open(p, f, 0o600)) as out:
            json.dump(config, out)
        process = subprocess.Popen(['node', '--import', 'tsx', 'scripts/rust/verify-pair-workflow.ts', str(settings)],
                                   cwd=Path(__file__).resolve().parent.parent,
                                   env={**os.environ, 'CREWROOM_RETAIN_FIXTURE': '1',
                                        'CREWROOM_NATIVE_TEST_BINARY': str(binaries / 'native-driver'),
                                        'CREWROOM_NATIVE_BINARY_DIRECTORY': str(binaries)})
        try:
            return process.wait(timeout=2400)
        except subprocess.TimeoutExpired:
            process.terminate()
            try:
                process.wait(timeout=60)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
                raise RuntimeError('Pair fixture exceeded cleanup deadline; inspect retained ownership')
            return 1
    finally:
        mail.stop()
        if mail.proc is not None:
            proof = mail.stopped_proof()
            (root / 'mail-cleanup.json').write_text(json.dumps(proof, indent=2) + '\n')
            if not proof['verifiedStopped']:
                raise RuntimeError('Owned Mail process group remains; restore is blocked')
        (root / 'mail-identity.json').write_text(json.dumps(mail.identity(), indent=2) + '\n')


if __name__ == '__main__':
    raise SystemExit(main())
