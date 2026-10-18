#!/usr/bin/env python3
"""Bounded opt-in FOR-14 production-service preflight, profile, recovery or matrix."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tempfile

PROJECT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(PROJECT / 'scripts'))
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
    parser.add_argument('--harness', choices=('codex', 'omp', 'claude-code'), required=True)
    parser.add_argument('--mode', choices=('preflight', 'smoke', 'matrix', 'recovery'), default='preflight')
    parser.add_argument('--harness-config', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--mail', type=Path, default=PROJECT / '.crewroom/experiments/rust-mail-prep/runtime/mcp-agent-mail')
    parser.add_argument('--roles', type=Path, default=Path.home() / '.config/crewroom/roles.json')
    parser.add_argument('--check-image', required=True)
    parser.add_argument('--binaries', type=Path, default=PROJECT / 'target/debug',
                        help='Native binaries to copy; reuse the first smoke bin directory for later profiles and matrix')
    parser.add_argument('--tool-profile-receipt', type=Path)
    args = parser.parse_args()
    # Prospective wall envelope includes serial admission/finalization overhead;
    # the service's per-cell and comparison active budgets remain unchanged.
    wall_limit_seconds = 5400 if args.mode == 'matrix' else 780 if args.mode == 'recovery' else 900
    cleanup_limit_seconds = 120
    if args.mode == 'matrix' and not args.tool_profile_receipt:
        parser.error('--mode matrix requires --tool-profile-receipt from a passing smoke')
    root = args.output.resolve()
    root.mkdir(mode=0o700, parents=True, exist_ok=False)
    binaries = root / 'bin'
    binaries.mkdir(mode=0o700)
    fingerprints = {}
    for name in ('crewroomd', 'crewroom-host', 'crewroom-process'):
        shutil.copy2(args.binaries.resolve() / name, binaries / name)
        fingerprints[name] = hashlib.sha256((binaries / name).read_bytes()).hexdigest()
    (root / 'binaries.json').write_text(json.dumps(fingerprints, indent=2) + '\n')
    runtime = Path(tempfile.mkdtemp(prefix='cr-job-'))
    config = private_config(args.harness_config)
    if config.get('kind') not in (None, args.harness):
        raise ValueError('Harness configuration kind differs; no silent route substitution')
    role_source = args.roles.resolve()
    if role_source.exists():
        shutil.copyfile(role_source, root / 'roles.json')
        (root / 'roles.json').chmod(0o600)
        for name in ('principles.json', 'playbooks.json'):
            if (role_source.parent / name).exists():
                shutil.copyfile(role_source.parent / name, root / name)
                (root / name).chmod(0o600)
        baseline = {'source': str(role_source), 'kind': 'saved-catalog', 'sha256': hashlib.sha256(role_source.read_bytes()).hexdigest()}
    else:
        # The daemon's Roles::read supplies its own effective defaults. Do not
        # invent a saved/custom revision or write the user's configuration.
        baseline = {'source': str(role_source), 'kind': 'effective-built-in-default', 'savedCatalogExists': False}
    source_identity = {
        'scope': 'launch-checkout-observation',
        'compiledBinaryProvenance': False,
        'note': 'These files were observed in the checkout when this helper launched. Copied executables may come from an earlier build; bind their hashes to the retained build/package evidence separately.',
        'head': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=PROJECT, text=True).strip(),
        'diffHash': hashlib.sha256(subprocess.check_output(['git', 'diff', 'HEAD'], cwd=PROJECT)).hexdigest(),
    }
    paths = subprocess.check_output(['git', 'ls-files', '-z', '--cached', '--others', '--exclude-standard'], cwd=PROJECT).decode().split('\0')
    source_files = [{'path': name, 'sha256': hashlib.sha256((PROJECT / name).read_bytes()).hexdigest()}
                    for name in sorted(set(paths)) if name and (PROJECT / name).is_file()]
    source_bytes = json.dumps(source_files, separators=(',', ':'), sort_keys=True).encode()
    (root / 'source-files.json').write_bytes(source_bytes)
    source_identity['sourceManifestHash'] = hashlib.sha256(source_bytes).hexdigest()
    mail = RustMail(root / 'mail', MailGuard(root, args.mail.resolve()), free_port(), binary=args.mail)
    try:
        mail.start()
        config.update(root=str(root), runtimeRoot=str(runtime), kind=args.harness,
                      evaluationImage=args.check_image, binaries=fingerprints,
                      wrapperWallSeconds=wall_limit_seconds,
                      wrapperCleanupSeconds=cleanup_limit_seconds,
                      baseline=baseline, sourceIdentity=source_identity,
                      mail={'url': f'http://127.0.0.1:{mail.port}/mcp/', 'token': mail.token,
                            'projectRoot': mail.project, 'retentionSeconds': 86400, 'atcDisabled': True})
        if args.tool_profile_receipt:
            config['toolProfileReceipt'] = str(args.tool_profile_receipt.resolve())
        settings = root / 'settings.json'
        with open(settings, 'x', opener=lambda p, f: os.open(p, f, 0o600)) as out:
            json.dump(config, out)
        process = subprocess.Popen(['node', '--import', 'tsx', 'scripts/rust/verify-comparison-workflow.ts', str(settings), args.mode],
                                   cwd=PROJECT, env={**os.environ, 'CREWROOM_NATIVE_BINARY_DIRECTORY': str(binaries)})
        try:
            return process.wait(timeout=wall_limit_seconds)
        except subprocess.TimeoutExpired:
            (root / 'termination-request.json').write_text(json.dumps({
                'cause': 'wrapper-wall-deadline', 'signal': 'SIGTERM',
                'wallSeconds': wall_limit_seconds,
            }, indent=2) + '\n')
            process.terminate()
            try:
                process.wait(timeout=cleanup_limit_seconds)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
                raise RuntimeError('Cleanup deadline exceeded; inspect retained process ownership before another trial')
            return 1
    finally:
        mail.stop()
        if mail.proc is not None:
            proof = mail.stopped_proof()
            (root / 'mail-cleanup.json').write_text(json.dumps(proof, indent=2) + '\n')
            if not proof['verifiedStopped']:
                raise RuntimeError('Owned Mail process group remains; another trial is blocked')
        (root / 'mail-identity.json').write_text(json.dumps(mail.identity(), indent=2) + '\n')
        (root / 'runtime-location.json').write_text(json.dumps({'root': str(runtime), 'retained': True}) + '\n')


if __name__ == '__main__':
    raise SystemExit(main())
