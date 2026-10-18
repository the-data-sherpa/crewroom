#!/usr/bin/env python3
"""Opt-in live intake and accepted plan handoff, production daemon and external Rust Mail.

Configuration is a private JSON file using existing disposable harness access.
No Jira or publication connection is configured. Native login files stay in place.
"""
import argparse
import json
import os
from pathlib import Path
import stat
import subprocess
from mailbox_contract_fixture import RustMail, free_port
from native_workflow_fixture import MailGuard


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--configuration', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--mail', type=Path, required=True)
    args = parser.parse_args()
    source = args.configuration.resolve()
    metadata = source.stat()
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_mode & 0o077 or metadata.st_uid != os.geteuid():
        raise ValueError('Fixture configuration must be a private owned file')
    config = json.loads(source.read_text())
    allowed = {'kind', 'model', 'codex', 'authFile', 'claude', 'nativeHome',
               'resolvedModel', 'omp', 'ompModels', 'catalogProfile'}
    if set(config) - allowed or config.get('kind') not in {'omp', 'codex', 'claude-code'}:
        raise ValueError('Invalid intake fixture configuration')
    root = args.output.resolve()
    root.mkdir(mode=0o700, parents=True, exist_ok=False)
    mail = RustMail(root / 'mail', MailGuard(root, args.mail.resolve()), free_port(), binary=args.mail)
    try:
        mail.start()
        config.update(root=str(root), intakeAcceptance=True,
                      mail={'url': f'http://127.0.0.1:{mail.port}/mcp/', 'token': mail.token,
                            'projectRoot': mail.project, 'retentionSeconds': 86400, 'atcDisabled': True})
        settings = root / 'settings.json'
        with open(settings, 'x', opener=lambda path, flags: os.open(path, flags, 0o600)) as output:
            json.dump(config, output)
        process = subprocess.Popen(
            ['node', 'node_modules/tsx/dist/cli.mjs', 'scripts/rust/verify-intake-workflow.ts', str(settings)],
            cwd=Path(__file__).resolve().parent.parent,
            env={**os.environ, 'CREWROOM_RETAIN_FIXTURE': '1'})
        try:
            return process.wait(timeout=900)
        except subprocess.TimeoutExpired:
            process.terminate()
            try:
                process.wait(timeout=60)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
                raise RuntimeError('Live workflow timed out; owned-process cleanup is unverified')
            return 1
    finally:
        mail.stop()
        (root / 'mail-identity.json').write_text(json.dumps(mail.identity(), indent=2) + '\n')


if __name__ == '__main__':
    raise SystemExit(main())
