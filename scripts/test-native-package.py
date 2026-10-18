#!/usr/bin/env python3
"""Regression for final native-package hashes using tiny compiled fixtures."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import shutil
import subprocess

PROJECT = Path(__file__).resolve().parent.parent
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
root = args.output.resolve(); root.mkdir(parents=True, exist_ok=False)
source = root / 'source'; source.mkdir(); (source / 'crates').mkdir()
c = source / 'crates/package-fixture.c'; c.write_text('int main(void) { return 0; }\n')
source_notices = {
    'LICENSE': 'crewroom/LICENSE',
    'THIRD_PARTY_NOTICES.md': 'crewroom/THIRD_PARTY_NOTICES.md',
    'crates/crewroom-core/licenses/V8-LICENSE': 'v8/V8-LICENSE',
    'crates/crewroom-core/licenses/V8-NOTICE': 'v8/V8-NOTICE',
}
for name in source_notices:
    destination = source / name
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(PROJECT / name, destination)
notices = {
    'gpui-libghostty': ['CREWROOM.md', 'LICENSE', 'vendor/ghostty/LICENSE', 'crewroom-patches.json'],
    'gpui-component': ['CREWROOM.md', 'LICENSE-APACHE', 'crewroom-patches.json'],
    'gpui-pre': ['CREWROOM.md', 'LICENSE-APACHE', 'crewroom-patches.json'],
}
for package, notice_names in notices.items():
    for name in notice_names:
        destination = source / 'vendor' / package / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(PROJECT / 'vendor' / package / name, destination)
for command in [['git', 'init', '-b', 'fixture'], ['git', 'add', '.'], ['git', '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-m', 'Synthetic package fixture']]:
    subprocess.run(command, cwd=source, check=True, capture_output=True)
binaries = root / 'binaries'; binaries.mkdir()
names = ['crewroom', 'crewroomd', 'crewroom-host', 'crewroom-process']
macos = platform.system() == 'Darwin'
subprocess.run(['/usr/bin/clang', *(['-arch', 'arm64'] if macos else []), str(c), '-o', str(binaries / names[0])], check=True)
for name in names[1:]: shutil.copy2(binaries / names[0], binaries / name)
bundle = root / ('Crewroom.app' if macos else 'Crewroom')
executable_relative = Path('Contents/MacOS') if macos else Path('bin')
resource_relative = Path('Contents/Resources') if macos else Path('share/crewroom')
command = ['python3', str(PROJECT / 'scripts/package-native.py'), '--binary-directory', str(binaries), '--source-root', str(source), '--output', str(bundle)]
result = subprocess.run(command, check=True, capture_output=True, text=True)
(root / 'package.log').write_text(result.stdout + result.stderr)
receipt = json.loads(Path(str(bundle) + '.receipt.json').read_text())
internal = json.loads((bundle / resource_relative / 'build-receipt.json').read_text())
for name, relative in source_notices.items():
    assert (bundle / resource_relative / 'licenses' / relative).read_bytes() == (source / name).read_bytes()
    assert receipt['sourceInputs'][name] == hashlib.sha256((source / name).read_bytes()).hexdigest()
for package, notice_names in notices.items():
    for name in notice_names:
        assert (bundle / resource_relative / 'licenses' / package / name).read_bytes() == (source / 'vendor' / package / name).read_bytes()
for name in names:
    binary = bundle / executable_relative / name
    assert receipt['binaries'][name]['packagedSha256'] == hashlib.sha256(binary.read_bytes()).hexdigest(), name
    assert receipt['binaries'][name]['bytes'] == binary.stat().st_size
    assert 'packagedSha256' not in internal['binaries'][name], 'Internal signature-dependent hash would be recursive.'
if macos:
    assert receipt['binaries']['crewroom']['preBundleSignatureSha256'] != receipt['binaries']['crewroom']['packagedSha256']
    subprocess.run(['codesign', '--verify', '--deep', '--strict', str(bundle)], check=True, capture_output=True)
else:
    assert receipt['packageFormat'] == 'linux-directory' and receipt['bundleSignatureVerified'] is False
    assert receipt['binaries']['crewroom']['inputSha256'] == receipt['binaries']['crewroom']['packagedSha256']
    assert (bundle / 'bin/crewroom-launcher').stat().st_mode & 0o111
assert subprocess.run(command, capture_output=True).returncode != 0, 'Existing package must not be replaced.'
assert receipt == json.loads(Path(str(bundle) + '.receipt.json').read_text())
# A missing license must fail before publishing any bundle or final receipt.
for index, name in enumerate(source_notices):
    notice = source / name
    saved = notice.read_bytes()
    notice.unlink()
    rejected = root / f'missing-notice-{index}'
    failed = subprocess.run([*command[:-1], str(rejected)], capture_output=True, text=True)
    notice.write_bytes(saved)
    assert failed.returncode != 0 and f'Missing source license notice: {name}' in failed.stderr
    assert not rejected.exists() and not Path(str(rejected) + '.receipt.json').exists()
report = {'passed': True, 'finalBinaryHashesMatch': 4, 'outerSignatureChangeReproduced': macos,
          'deepSignatureVerified': macos, 'packageFormat': receipt['packageFormat'], 'existingOutputRefused': True,
          'vendoredDependencyNoticesPreserved': sum(map(len, notices.values())),
          'sourceNoticesPreserved': len(source_notices), 'missingSourceNoticesRefused': len(source_notices),
          'scope': 'Synthetic native executable packaging regression; no Crewroom workflow, GUI or release acceptance claim.'}
(root / 'report.json').write_text(json.dumps(report, indent=2) + '\n'); print(json.dumps(report))
