#!/usr/bin/env python3
"""Revalidate an MH6 live candidate after a check-runtime repair, preserving failed evidence."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
from mailbox_contract_fixture import RustMail, free_port
from native_workflow_fixture import MailGuard


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--seed', type=Path, required=True)
    parser.add_argument('--mail', type=Path, required=True)
    parser.add_argument('--name', required=True)
    parser.add_argument('--repair-ui', action='store_true')
    args = parser.parse_args()
    seed = args.seed.resolve()
    if seed.parent != Path('/tmp') or not seed.name.startswith('cr-mh6-'):
        raise ValueError('Use a disposable MH6 seed')
    if not args.name.replace('-', '').isalnum():
        raise ValueError('Use a simple receipt name')
    prior = json.loads((seed / 'pair-result.json').read_text())
    fixture = Path(prior['fixtureRoot']).resolve()
    if fixture.parent != seed or (fixture / 'data/native-connection.json').exists():
        raise ValueError('Stop the owned seed daemon first')
    proof = json.loads((seed / 'mail-cleanup.json').read_text())
    if not proof.get('verifiedStopped'):
        raise ValueError('Mail cleanup proof required')
    try:
        os.killpg(proof['pid'], 0)
    except ProcessLookupError:
        pass
    else:
        raise ValueError('Previous Mail group still exists')
    output = seed / args.name
    output.mkdir(mode=0o700, exist_ok=False)
    binaries = output / 'bin'
    binaries.mkdir()
    sources = {n: Path('target/debug') / n for n in ('crewroomd', 'crewroom-host', 'crewroom-process')}
    sources['native-driver'] = Path(os.environ['CREWROOM_NATIVE_TEST_BINARY'])
    for name, source in sources.items():
        shutil.copy2(source, binaries / name)
    hashes = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in binaries.iterdir()}
    (output / 'binaries.json').write_text(json.dumps(hashes, indent=2) + '\n')
    shutil.copy2(seed / 'mail/calls.json', output / 'prior-mail-calls.json')
    mail = RustMail(seed / 'mail', MailGuard(seed, args.mail.resolve()), free_port(), binary=args.mail)
    try:
        mail.start()
        setup = {'seed': str(seed), 'output': str(output), 'fixture': str(fixture), 'repairUi': args.repair_ui,
                 'mail': {'url': f'http://127.0.0.1:{mail.port}/mcp/', 'token': mail.token,
                          'projectRoot': mail.project, 'retentionSeconds': 86400, 'atcDisabled': True}}
        settings = output / 'settings.json'
        with open(settings, 'x', opener=lambda p, f: os.open(p, f, 0o600)) as stream:
            json.dump(setup, stream)
        return subprocess.call(['node', '--import', 'tsx', 'scripts/rust/verify-pair-review.ts', str(settings)],
                               env={**os.environ, 'CREWROOM_NATIVE_BINARY_DIRECTORY': str(binaries),
                                    'CREWROOM_NATIVE_TEST_BINARY': str(binaries / 'native-driver')})
    finally:
        mail.stop()
        if mail.proc is not None:
            proof = mail.stopped_proof()
            for path in (output / 'mail-cleanup.json', seed / 'mail-cleanup.json'):
                path.write_text(json.dumps(proof, indent=2) + '\n')
            if not proof['verifiedStopped']:
                raise RuntimeError('Owned Mail process group remains')


if __name__ == '__main__':
    raise SystemExit(main())
