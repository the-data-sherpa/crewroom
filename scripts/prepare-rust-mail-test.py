#!/usr/bin/env python3
"""Download the pinned host-platform test release into ignored experiment state."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import tarfile
import urllib.request

PROJECT = Path(__file__).resolve().parent.parent
VERSION = 'v0.3.36'
# v0.3.36's published SHA256SUMS; these are build fixtures, not system installs.
RELEASES = {
    ('Darwin', 'arm64'): ('aarch64-apple-darwin', '2b90585878dd3c8509c21b177450f9f975caebb27a2f6a8601efed0ffb016523'),
    ('Linux', 'x86_64'): ('x86_64-unknown-linux-gnu', 'c80943427f434a4b98f8c6a7e5a16440271ed9f3fa9f0825f150c9013c64325e'),
    ('Linux', 'aarch64'): ('aarch64-unknown-linux-gnu', '977d76f2e5e12f4cd4afd4b205d5b74d6b69401dd5db91cf08880bf88dade068'),
}
BASE = 'https://github.com/Dicklesworthstone/mcp_agent_mail_rust/releases/download/' + VERSION


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=PROJECT / '.crewroom/experiments/rust-mail-prep')
    args = parser.parse_args()
    target = RELEASES.get((platform.system(), platform.machine()))
    if target is None:
        parser.error('Test downloads support Apple Silicon macOS and glibc Linux x86_64/aarch64.')
    triple, sha256 = target
    name = f'mcp-agent-mail-{triple}.tar.xz'
    root = args.output.resolve(); root.mkdir(parents=True, exist_ok=True, mode=0o700)
    archive = root / name
    if not archive.exists():
        pending = root / (name + '.part')
        urllib.request.urlretrieve(BASE + '/' + name, pending)
        if hashlib.sha256(pending.read_bytes()).hexdigest() != sha256:
            pending.unlink()
            raise RuntimeError('Release archive checksum mismatch')
        pending.rename(archive)
    if hashlib.sha256(archive.read_bytes()).hexdigest() != sha256:
        raise RuntimeError('Cached release archive checksum mismatch')
    runtime = root / 'runtime'
    if runtime.exists():
        if not (runtime / 'mcp-agent-mail').is_file():
            raise RuntimeError('Existing runtime is incomplete; use a fresh output directory')
    else:
        with tarfile.open(archive) as bundle:
            members = bundle.getmembers()
            if {m.name for m in members} != {'am', 'mcp-agent-mail', 'README.md', 'LICENSE'}:
                raise RuntimeError('Unexpected archive contents')
            if any(not m.isfile() for m in members):
                raise RuntimeError('Unexpected archive member type')
            bundle.extractall(runtime, filter='data')
    # Verify even an existing extracted binary against the pinned archive.
    with tarfile.open(archive) as bundle:
        expected = hashlib.sha256(bundle.extractfile('mcp-agent-mail').read()).hexdigest()
    actual = hashlib.sha256((runtime / 'mcp-agent-mail').read_bytes()).hexdigest()
    if actual != expected:
        raise RuntimeError('Extracted executable differs from the pinned archive')
    identity = {'release': VERSION, 'target': triple, 'archiveSha256': sha256, 'binarySha256': actual,
                'url': BASE + '/' + name, 'scope': 'Local experimental download; no global installation'}
    (root / 'verified-release.json').write_text(json.dumps(identity, indent=2) + '\n')
    print(json.dumps(identity))


if __name__ == '__main__':
    main()
