#!/usr/bin/env python3
"""Install the pinned native build toolchain into a private local directory."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request

VERSION = '0.16.0'
# Published hashes: https://ziglang.org/download/index.json (0.16.0).
TOOLCHAINS = {
    ('Darwin', 'arm64'): ('aarch64-macos', 'b23d70deaa879b5c2d486ed3316f7eaa53e84acf6fc9cc747de152450d401489'),
    ('Linux', 'x86_64'): ('x86_64-linux', '70e49664a74374b48b51e6f3fdfbf437f6395d42509050588bd49abe52ba3d00'),
    ('Linux', 'aarch64'): ('aarch64-linux', 'ea4b09bfb22ec6f6c6ceac57ab63efb6b46e17ab08d21f69f3a48b38e1534f17'),
}
ROOT = Path(__file__).resolve().parent.parent


def digest(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def inventory(root):
    return {str(path.relative_to(root)): digest(path) for path in sorted(root.rglob('*')) if path.is_file()}


def check_linux_libraries(parser):
    modules = ['fontconfig', 'freetype2', 'libxml-2.0', 'xkbcommon', 'xkbcommon-x11', 'wayland-client', 'egl']
    missing = [module for module in modules if subprocess.run(
        ['pkg-config', '--exists', module], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode]
    if missing:
        parser.error(f'Missing Linux development libraries: {", ".join(missing)}. See docs/linux.md for the Arch package list.')
    with tempfile.TemporaryDirectory(prefix='crewroom-link-check-') as temporary:
        result = subprocess.run(['clang', '-x', 'c', '-', '-o', str(Path(temporary) / 'probe'), '-lc++'],
                                input='int main(void) { return 0; }\n', text=True, capture_output=True)
    if result.returncode:
        parser.error('The Linux GUI requires the LLVM libc++ runtime (LLVM 21 or newer). On Arch install libc++; see docs/linux.md. Compiler link probe failed: ' + result.stderr.strip())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--directory', type=Path, default=ROOT / '.crewroom/toolchains')
    parser.add_argument('--check-build-dependencies', action='store_true', help='Check Linux development libraries and the libc++ linker input before building.')
    args = parser.parse_args()
    if sys.version_info < (3, 12):
        parser.error('Python 3.12 or newer is required for verified archive extraction.')
    system = platform.system()
    toolchain = TOOLCHAINS.get((system, platform.machine()))
    if toolchain is None:
        parser.error('Native builds support Apple Silicon macOS and x86_64/aarch64 Linux.')
    target_platform, sha256 = toolchain
    archive_name = f'zig-{target_platform}-{VERSION}'
    url = f'https://ziglang.org/download/{VERSION}/{archive_name}.tar.xz'
    if system == 'Darwin':
        for tool in ['clang', 'libtool']:
            subprocess.run(['xcrun', '--find', tool], check=True, stdout=subprocess.DEVNULL)
        subprocess.run(['xcrun', '--show-sdk-path'], check=True, stdout=subprocess.DEVNULL)
    else:
        missing = [name for name in ['cc', 'clang', 'pkg-config'] if shutil.which(name) is None]
        if missing:
            parser.error(f'Missing Linux build tools: {", ".join(missing)}. On Arch install base-devel clang pkgconf; see docs/linux.md for native library dependencies.')
        if args.check_build_dependencies:
            check_linux_libraries(parser)
    # rustup honors rust-toolchain.toml when installed; distro Cargo uses its
    # installed compiler. Neither path changes the global toolchain.
    subprocess.run(['cargo', '--version'], cwd=ROOT, check=True, stdout=subprocess.DEVNULL)
    directory = args.directory.expanduser().resolve()
    directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    target = directory / archive_name
    receipt = directory / f'{archive_name}.json'
    if target.exists():
        expected = json.loads(receipt.read_text())
        if expected['archiveSha256'] != sha256 or inventory(target) != expected['files']:
            raise RuntimeError('Existing Zig toolchain differs from its verified receipt. Preserve it and select a new directory.')
    else:
        with tempfile.TemporaryDirectory(prefix='zig-download-', dir=directory) as temporary:
            staging = Path(temporary)
            archive = staging / 'zig.tar.xz'
            with urllib.request.urlopen(url, timeout=120) as source, archive.open('wb') as out:
                shutil.copyfileobj(source, out)
            if digest(archive) != sha256:
                raise RuntimeError('Pinned Zig archive SHA256 does not match.')
            with tarfile.open(archive, 'r:xz') as source:
                source.extractall(staging, filter='data')
            candidate = staging / archive_name
            if subprocess.check_output([str(candidate / 'zig'), 'version'], text=True).strip() != VERSION:
                raise RuntimeError('Pinned Zig version does not match.')
            files = inventory(candidate)
            os.rename(candidate, target)
            receipt.write_text(json.dumps({'version': VERSION, 'url': url, 'archiveSha256': sha256, 'files': files}, indent=2) + '\n')
            receipt.chmod(0o600)
    if subprocess.check_output([str(target / 'zig'), 'version'], text=True).strip() != VERSION:
        raise RuntimeError('Installed Zig version does not match.')
    print(target / 'zig')


if __name__ == '__main__':
    main()
