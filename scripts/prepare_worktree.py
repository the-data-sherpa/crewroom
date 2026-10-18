#!/usr/bin/env python3
"""Make a fresh Crewroom checkout or disposable worktree self-sufficient for verification (FOR-59).

Idempotent. Reuses the fixture-only OMP and the pinned Rust Mail test runtime that another
checkout already installed, by linking them under this checkout's ignored `.crewroom/`.
Nothing is copied and no private state (provider logins, sessions, stores) is touched or
tracked. Without an installed source, `--install-missing` installs the pinned fixtures.

    python3 scripts/prepare_worktree.py               # link fixtures, npm ci if node_modules is absent
    python3 scripts/prepare_worktree.py --check       # report only; exit 1 with one message when incomplete
    python3 scripts/prepare_worktree.py --remove      # remove only the links this step created
"""
import argparse
import os
from pathlib import Path
import subprocess
import sys

PROJECT = Path(__file__).resolve().parent.parent
STEP = 'python3 scripts/prepare_worktree.py'
OMP_VERSION = '18.4.12'


class Fixture:
    def __init__(self, name, path, marker, install, platforms=None):
        self.name, self.path, self.marker, self.install, self.platforms = name, Path(path), Path(marker), install, platforms

    def required(self):
        return self.platforms is None or sys.platform in self.platforms

    def ready(self, root):
        return (root / self.path / self.marker).is_file()


FIXTURES = (
    # Linux browser fixtures run this installation under bubblewrap; macOS uses the installed `omp`.
    Fixture('fixture-only OMP ' + OMP_VERSION, '.crewroom/mh2-omp-fixture', 'node_modules/@oh-my-pi/pi-coding-agent/dist/cli.js',
            [['npm', 'install', '--prefix', '.crewroom/mh2-omp-fixture', '--no-audit', '--no-fund', '--ignore-scripts',
              f'@oh-my-pi/pi-coding-agent@{OMP_VERSION}']], platforms=('linux',)),
    Fixture('Rust Mail test runtime', '.crewroom/experiments/rust-mail-prep', 'runtime/mcp-agent-mail',
            [[sys.executable, 'scripts/prepare-rust-mail-test.py']]),
)


def missing_message(fixture, root=PROJECT):
    """The single actionable message every fixture consumer raises when a fixture is absent."""
    return (f'Missing {fixture.name} at {root / fixture.path / fixture.marker}. '
            f'Run `{STEP}` (links an installed checkout\'s fixture) or `{STEP} --install-missing`; '
            'see docs/testing/feature-map/README.md#fresh-worktrees.')


def fixture(name_prefix):
    return next(f for f in FIXTURES if f.name.startswith(name_prefix))


def git(root, *args):
    return subprocess.run(['git', '-C', str(root), *args], capture_output=True, text=True)


def default_source(root):
    """The main checkout of this repository: the first entry of `git worktree list`."""
    listed = git(root, 'worktree', 'list', '--porcelain')
    for line in listed.stdout.splitlines():
        if line.startswith('worktree '):
            source = Path(line.removeprefix('worktree ')).resolve()
            return None if source == root.resolve() else source
    return None


def ignored(root, path):
    return git(root, 'check-ignore', '-q', '--no-index', str(path)).returncode == 0


def ensure_parent_private(target):
    target.parent.mkdir(parents=True, exist_ok=True, mode=0o700)


def plan(root, source):
    """Classify each required fixture as ready, linkable or missing; never mutates."""
    rows = []
    for item in FIXTURES:
        if not item.required():
            continue
        if item.ready(root):
            rows.append((item, 'ready', None))
        elif source and item.ready(source):
            rows.append((item, 'link', (source / item.path).resolve()))
        else:
            rows.append((item, 'missing', None))
    return rows


def prepare(root, source, install_missing=False, out=print):
    failures = []
    for item, state, origin in plan(root, source):
        target = root / item.path
        if state == 'ready':
            out(f'ready    {item.name}: {item.path}')
            continue
        if not ignored(root, target):
            failures.append(f'{item.path} is not ignored by Git; refusing to create fixture state there.')
            continue
        if state == 'link':
            if target.is_symlink():
                target.unlink()  # a stale link from an earlier preparation
            elif target.exists():
                failures.append(f'{item.path} exists but is incomplete; remove it or reinstall the fixture there.')
                continue
            ensure_parent_private(target)
            target.symlink_to(origin, target_is_directory=True)
            out(f'linked   {item.name}: {item.path} -> {origin}')
        elif install_missing:
            ensure_parent_private(target)
            for command in item.install:
                subprocess.run(command, cwd=root, check=True)
            if not item.ready(root):
                failures.append(missing_message(item, root))
            else:
                out(f'installed {item.name}: {item.path}')
        else:
            failures.append(missing_message(item, root))
    return failures


def remove(root, source, out=print):
    """Remove only symlinks that point at the source checkout's fixtures; installed copies stay."""
    for item in FIXTURES:
        target = root / item.path
        if target.is_symlink() and (source is None or Path(os.readlink(target)).resolve() == (source / item.path).resolve()):
            target.unlink()
            out(f'removed  {item.path}')


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--source', type=Path, help='checkout whose installed fixtures to link (default: the main Git worktree)')
    parser.add_argument('--check', action='store_true', help='report readiness without changing anything')
    parser.add_argument('--install-missing', action='store_true', help='install pinned fixtures that no source checkout provides (network)')
    parser.add_argument('--build-helpers', action='store_true', help='also build crewroom-process and crewroom-host into CARGO_TARGET_DIR')
    parser.add_argument('--no-npm', action='store_true', help='do not run npm ci when node_modules is absent')
    parser.add_argument('--remove', action='store_true', help='remove the fixture links this step created')
    args = parser.parse_args(argv)
    root = PROJECT
    source = args.source.resolve() if args.source else default_source(root)
    if args.remove:
        remove(root, source)
        return 0
    if args.check:
        failures = [missing_message(item, root) for item, state, _ in plan(root, source) if state != 'ready']
        if not (root / 'node_modules').is_dir():
            failures.append(f'node_modules is absent. Run `{STEP}` or `npm ci`.')
        if failures:
            print('\n'.join(failures), file=sys.stderr)
            return 1
        print('Fresh-worktree preparation is complete.')
        return 0
    if not (root / 'node_modules').is_dir() and not args.no_npm:
        subprocess.run(['npm', 'ci'], cwd=root, check=True)
    failures = prepare(root, source, args.install_missing)
    target_dir = Path(os.environ.get('CARGO_TARGET_DIR') or root / 'target')
    if args.build_helpers:
        subprocess.run(['cargo', 'build', '--locked', '-p', 'crewroom-runtime', '--bins'], cwd=root, check=True,
                       env={**os.environ, 'CARGO_TARGET_DIR': str(target_dir)})
    if target_dir.resolve() != (root / 'target').resolve():
        print(f'note     CARGO_TARGET_DIR={target_dir} is outside this worktree; parallel worktrees sharing it produce stale artifacts.')
    print(f'cargo    use a private build directory: export CARGO_TARGET_DIR="{root / "target"}"')
    if failures:
        print('\n'.join(failures), file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
