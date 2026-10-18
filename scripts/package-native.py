#!/usr/bin/env python3
"""Package the four already-built native executables; never start runtime state."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import plistlib
import re
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent.parent
BINARIES = ['crewroom', 'crewroomd', 'crewroom-process', 'crewroom-host']


def run(*args, cwd=ROOT):
    return subprocess.check_output(args, cwd=cwd, text=True, stderr=subprocess.STDOUT).strip()


def digest(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def inspect_binary(binary, system, machine):
    if not binary.is_file() or binary.is_symlink() or not binary.stat().st_mode & 0o111:
        raise ValueError(f'{binary.name} must be a regular executable file.')
    description = run('/usr/bin/file', str(binary))
    if system == 'Darwin':
        if 'Mach-O 64-bit executable arm64' not in description:
            raise ValueError(f'{binary.name} must be an Apple Silicon Mach-O executable.')
        dependencies = [line.strip().split(' (')[0] for line in run('/usr/bin/otool', '-L', str(binary)).splitlines()[1:]]
        system_paths = ('/usr/lib/', '/System/Library/')
    else:
        architecture = {'x86_64': 'x86-64', 'aarch64': 'ARM aarch64'}[machine]
        if 'ELF 64-bit' not in description or 'executable' not in description or architecture not in description:
            raise ValueError(f'{binary.name} must be a native {machine} Linux ELF executable.')
        # These are locally built inputs, not arbitrary downloaded executables.
        listing = run('/usr/bin/ldd', str(binary))
        if 'not found' in listing:
            raise ValueError(f'{binary.name} has missing shared libraries: {listing}')
        dependencies = re.findall(r'(?:=>[ \t]+|^[ \t]*)(/[^\n]*?)\s+\(', listing, re.MULTILINE)
    if system == 'Darwin':
        external = [path for path in dependencies if not path.startswith(system_paths)]
        if external:
            raise ValueError(f'{binary.name} has unbundled dependencies: {external}')
    return dependencies


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary-directory', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--source-root', type=Path, default=ROOT, help='Source checkout used to build these binaries.')
    options = parser.parse_args()
    system, machine = platform.system(), platform.machine()
    if (system, machine) not in [('Darwin', 'arm64'), ('Linux', 'x86_64'), ('Linux', 'aarch64')]:
        parser.error('Native packaging supports Apple Silicon macOS and x86_64/aarch64 Linux.')
    macos_package = system == 'Darwin'
    directory = options.binary_directory.resolve()
    source_root = options.source_root.resolve()
    output = options.output.resolve()
    final_receipt = Path(str(output) + '.receipt.json')
    if output.exists() or final_receipt.exists():
        parser.error('Output already exists. Choose a new path or preserve/remove the old build explicitly.')
    binaries = {}
    for name in BINARIES:
        binary = directory / name
        try:
            dependencies = inspect_binary(binary, system, machine)
        except ValueError as error:
            parser.error(str(error))
        binaries[name] = {'inputSha256': digest(binary), 'dependencies': dependencies}
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='crewroom-package-', dir=output.parent) as temporary:
        bundle = Path(temporary) / ('Crewroom.app' if macos_package else 'Crewroom')
        contents = bundle / 'Contents'
        executables = contents / 'MacOS' if macos_package else bundle / 'bin'
        resources = contents / 'Resources' if macos_package else bundle / 'share/crewroom'
        executables.mkdir(parents=True)
        resources.mkdir(parents=True)
        for name in BINARIES:
            shutil.copy2(directory / name, executables / name)
            if macos_package:
                subprocess.run(['codesign', '--force', '--sign', '-', str(executables / name)], check=True)
        launcher = ROOT / 'scripts/launch-native.sh'
        shutil.copy2(launcher, executables / 'crewroom-launcher')
        (executables / 'crewroom-launcher').chmod(0o755)
        plist = {'CFBundleExecutable': 'crewroom', 'CFBundleIdentifier': 'local.crewroom.native',
                 'CFBundleName': 'Crewroom', 'CFBundleDisplayName': 'Crewroom', 'CFBundlePackageType': 'APPL',
                 'CFBundleVersion': '2', 'CFBundleShortVersionString': '0.2.0',
                 'LSMinimumSystemVersion': '15.0', 'NSHighResolutionCapable': True}
        if macos_package:
            subprocess.run(['codesign', '--force', '--sign', '-', str(executables / 'crewroom-launcher')], check=True)
            (contents / 'Info.plist').write_bytes(plistlib.dumps(plist))
        source_notices = {
            'LICENSE': 'crewroom/LICENSE',
            'THIRD_PARTY_NOTICES.md': 'crewroom/THIRD_PARTY_NOTICES.md',
            'crates/crewroom-core/licenses/V8-LICENSE': 'v8/V8-LICENSE',
            'crates/crewroom-core/licenses/V8-NOTICE': 'v8/V8-NOTICE',
        }
        for name, relative in source_notices.items():
            source = source_root / name
            if not source.is_file():
                raise RuntimeError(f'Missing source license notice: {name}')
            destination = resources / 'licenses' / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, destination)
        required_notices = {
            'gpui-libghostty': ['CREWROOM.md', 'LICENSE', 'vendor/ghostty/LICENSE', 'crewroom-patches.json'],
            'gpui-component': ['CREWROOM.md', 'LICENSE-APACHE', 'crewroom-patches.json'],
            'gpui-pre': ['CREWROOM.md', 'LICENSE-APACHE', 'crewroom-patches.json'],
        }
        for package, required in required_notices.items():
            vendor = source_root / 'vendor' / package
            notices = resources / 'licenses' / package
            for name in required:
                if not (vendor / name).is_file():
                    raise RuntimeError(f'Missing vendored dependency notice: {package}/{name}')
            # Preserve all upstream license/notice files with their relative paths.
            for path in sorted(vendor.rglob('*')):
                if path.is_file() and (path.name.upper().startswith(('LICENSE', 'COPYING', 'NOTICE')) or path.name in ['CREWROOM.md', 'VENDOR.md', 'crewroom-patches.json']):
                    destination = notices / path.relative_to(vendor)
                    destination.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(path, destination)
        if macos_package:
            for name in BINARIES:
                binaries[name]['preBundleSignatureSha256'] = digest(executables / name)
                binaries[name]['preBundleSignatureBytes'] = (executables / name).stat().st_size
        source_paths = run('git', 'ls-files', '--cached', '--others', '--exclude-standard', cwd=source_root).splitlines()
        source_inputs = {name: digest(source_root / name) for name in sorted(set(source_paths))
                         if (name in ['Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'LICENSE', 'THIRD_PARTY_NOTICES.md'] or name.startswith(('crates/', 'vendor/', 'scripts/')))
                         and (source_root / name).is_file()}
        receipt = {'launcherSha256': digest(launcher), 'sourceInputs': source_inputs, 'sourceRevision': run('git', 'rev-parse', 'HEAD', cwd=source_root), 'packagerSha256': digest(Path(__file__)),
                   'workingTreeDiffSha256': hashlib.sha256(subprocess.check_output(['git', 'diff', 'HEAD', '--binary'], cwd=source_root)).hexdigest(),
                   'rustc': run('rustc', '--version'), 'platform': run('sw_vers') if macos_package else platform.platform(), 'architecture': machine, 'binaries': binaries,
                   'packageFormat': 'macos-app' if macos_package else 'linux-directory',
                   'signing': 'Local ad-hoc signatures; no distribution notarization claim.' if macos_package else 'Unsigned local Linux directory package; shared libraries must be supplied by the host.',
                   'scope': 'Source provenance. Final binary hashes are in the adjacent .receipt.json sidecar; full GUI/runtime acceptance still requires exercising this package.'}
        (resources / 'build-receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
        if macos_package:
            subprocess.run(['codesign', '--force', '--sign', '-', str(bundle)], check=True)
            subprocess.run(['codesign', '--verify', '--deep', '--strict', str(bundle)], check=True)
        executable_relative = executables.relative_to(bundle)
        resource_relative = resources.relative_to(bundle)
        bundle.rename(output)
    # Signing the outer bundle changes its main executable. A final executable
    # hash cannot live inside its own signed resources without a hash cycle.
    for name in BINARIES:
        binary = output / executable_relative / name
        binaries[name]['packagedSha256'] = digest(binary)
        binaries[name]['bytes'] = binary.stat().st_size
    receipt['bundleSignatureVerified'] = macos_package
    receipt['internalProvenanceSha256'] = digest(output / resource_relative / 'build-receipt.json')
    receipt['scope'] = 'Final package hashes; full GUI/runtime acceptance requires exercising this package.'
    final_receipt.write_text(json.dumps(receipt, indent=2) + '\n')
    print(json.dumps({'bundle': str(output), 'receipt': str(final_receipt), 'binaryBytes': sum(value['bytes'] for value in binaries.values())}))


if __name__ == '__main__':
    main()
