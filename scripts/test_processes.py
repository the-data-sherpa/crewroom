#!/usr/bin/env python3
"""List or stop this checkout's test processes without process-name/group kills."""
import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import signal
import stat
import subprocess
import sys
import tempfile
import time
import uuid

REPO = Path(__file__).resolve().parents[1]
RUN_KEY = 'CREWROOM_TEST_RUN'
REPO_KEY = 'CREWROOM_TEST_REPO'
PATH_KEYS = {'CREWROOM_DATA_DIR', 'CREWROOM_NATIVE_DATA_DIR', 'CREWROOM_E2E_DATA',
             'CREWROOM_RUNTIME_CONFIG', 'PI_CODING_AGENT_DIR',
             'PI_CODING_AGENT_SESSION_DIR', 'TMPDIR', 'STORAGE_ROOT'}


class BsdInfo(ctypes.Structure):
    # Darwin's public sys/proc_info.h, PROC_PIDTBSDINFO. Start time is microseconds,
    # unlike ps lstart (seconds). No process environment/arguments are printed.
    _fields_ = [(name, ctypes.c_uint32) for name in (
        'flags', 'status', 'xstatus', 'pid', 'ppid', 'uid', 'gid', 'ruid', 'rgid',
        'svuid', 'svgid', 'reserved')]
    _fields_ += [('comm', ctypes.c_char * 16), ('name', ctypes.c_char * 32)]
    _fields_ += [(name, ctypes.c_uint32) for name in
                 ('nfiles', 'pgid', 'jobc', 'tdev', 'tpgid', 'nice')]
    _fields_ += [('start_sec', ctypes.c_uint64), ('start_usec', ctypes.c_uint64)]


class Processes:
    def __init__(self):
        if sys.platform == 'darwin':
            self.lib = ctypes.CDLL('/usr/lib/libproc.dylib', use_errno=True)
            self.sysctl = ctypes.CDLL(None, use_errno=True).sysctl
            self.lib.proc_pidinfo.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_uint64,
                                             ctypes.c_void_p, ctypes.c_int]
            self.lib.proc_pidpath.argtypes = [ctypes.c_int, ctypes.c_void_p, ctypes.c_uint32]
            self.sysctl.argtypes = [ctypes.POINTER(ctypes.c_int), ctypes.c_uint,
                                   ctypes.c_void_p, ctypes.POINTER(ctypes.c_size_t),
                                   ctypes.c_void_p, ctypes.c_size_t]
        elif not sys.platform.startswith('linux'):
            raise ValueError('Supported process inspection: macOS and Linux only.')

    def pids(self):
        return [int(p) for p in subprocess.check_output(
            ['/bin/ps', '-axo', 'pid='], text=True).split()]

    def read(self, pid):
        try:
            if sys.platform == 'darwin':
                info = BsdInfo()
                size = self.lib.proc_pidinfo(pid, 3, 0, ctypes.byref(info), ctypes.sizeof(info))
                if size != ctypes.sizeof(info) or info.uid != os.getuid() or info.status == 5:
                    return None
                binary = ctypes.create_string_buffer(4096)
                if self.lib.proc_pidpath(pid, binary, len(binary)) <= 0:
                    return None
                # KERN_PROCARGS2 returns argc, executable, argv, then environment.
                mib = (ctypes.c_int * 3)(1, 49, pid)
                length = ctypes.c_size_t()
                if self.sysctl(mib, 3, None, ctypes.byref(length), None, 0) != 0:
                    return None
                if not 4 < length.value <= 2 * 1024 * 1024:
                    return None
                raw = ctypes.create_string_buffer(length.value)
                if self.sysctl(mib, 3, raw, ctypes.byref(length), None, 0) != 0:
                    return None
                data = raw.raw[:length.value]
                argc = int.from_bytes(data[:4], sys.byteorder, signed=True)
                if not 0 < argc < 100000:
                    return None
                position = data.index(b'\0', 4) + 1
                while position < len(data) and data[position] == 0:
                    position += 1
                fields = data[position:].split(b'\0')
                argv = fields[:argc]
                environ = fields[argc:]
                born = f'{info.start_sec}.{info.start_usec:06d}'
                parent, executable = info.ppid, os.fsdecode(binary.value)
            else:
                proc = Path('/proc') / str(pid)
                if proc.stat().st_uid != os.getuid():
                    return None
                fields = (proc / 'stat').read_text().rsplit(')', 1)[1].split()
                if fields[0] == 'Z':
                    return None
                parent, born = int(fields[1]), fields[19]
                executable = os.readlink(proc / 'exe')
                argv = (proc / 'cmdline').read_bytes().split(b'\0')[:-1]
                environ = (proc / 'environ').read_bytes().split(b'\0')
            env = {}
            for item in environ:
                key, sep, value = item.partition(b'=')
                # Retain only scope markers and paths, never credentials.
                if sep and os.fsdecode(key) in PATH_KEYS | {RUN_KEY, REPO_KEY}:
                    env[os.fsdecode(key)] = os.fsdecode(value)
            args = [os.fsdecode(a) for a in argv]
            identity = (pid, born, executable, hashlib.sha256(b'\0'.join(argv)).hexdigest())
            return {'pid': pid, 'parent': parent, 'born': born, 'executable': executable,
                    'args': args, 'env': env, 'identity': identity}
        except (OSError, ValueError, IndexError):
            return None


def beneath(path, root):
    return path == root or root in path.parents


def private_json(path):
    """A registry record is a local ownership assertion, not an executable command."""
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        raise ValueError('Test registry records must be private regular files owned by this user.')
    return json.loads(path.read_text())


def registry(repo, create=False):
    root = repo / '.crewroom' / 'test-processes'
    if root.is_symlink() or (repo / '.crewroom').is_symlink():
        raise ValueError('Refusing a symlinked test registry.')
    if create:
        root.mkdir(parents=True, exist_ok=True, mode=0o700)
    if root.exists():
        info = root.stat()
        if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
            raise ValueError('Test registry directory must be private and owned by this user.')
    return root


def protected_roots(repo):
    return [(repo / '.crewroom' / name).resolve() for name in ('rust', 'pilot')]


def fixture_root(value, repo):
    root = Path(value).expanduser().resolve()
    bases = [(repo / '.crewroom' / 'test-results').resolve(),
             Path(tempfile.gettempdir()).resolve(), Path('/tmp').resolve()]
    if any(beneath(root, p) or beneath(p, root) for p in protected_roots(repo)):
        raise ValueError('Normal .crewroom/rust and retained pilot data are protected.')
    if not any(beneath(root, b) and root != b for b in bases):
        raise ValueError('--root must name a specific fixture below the test-results or temporary directory.')
    if root.exists() and (not root.is_dir() or root.stat().st_uid != os.getuid()):
        raise ValueError('Fixture directory must belong to this user.')
    return root


def load_runs(repo):
    runs = {}
    for path in registry(repo).glob('*.json'):
        record = private_json(path)
        if not isinstance(record, dict) or record.get('repo') != str(repo) or record.get('id') != path.stem:
            raise ValueError('Test registry identity mismatch.')
        uuid.UUID(record['id'])
        roots = record.get('roots', [])
        if not isinstance(roots, list) or any(not isinstance(p, str) for p in roots):
            raise ValueError('Invalid fixture roots in test registry.')
        record['roots'] = [fixture_root(p, repo) for p in roots]
        runs[record['id']] = record
    return runs


def references(process):
    values = process['args'][1:] + [v for k, v in process['env'].items() if k in PATH_KEYS]
    for value in values:
        if value.startswith('--') and '=' in value:
            value = value.split('=', 1)[1]
        if value.startswith('/') and '\n' not in value and '\0' not in value:
            yield Path(value).resolve()


def discover(probe, repo, roots=(), run_id=None):
    runs = load_runs(repo)
    if run_id and run_id not in runs:
        raise ValueError('Unknown test run ID for this checkout.')
    allowed = list(roots)
    if not run_id:
        allowed.append((repo / '.crewroom' / 'test-results').resolve())
    for ident, record in runs.items():
        if not run_id or ident == run_id:
            allowed.extend(record['roots'])
    table = {p['pid']: p for pid in probe.pids() if (p := probe.read(pid))}
    excluded = {1, os.getpid()}
    parent = table.get(os.getpid(), {}).get('parent')
    while parent and parent not in excluded:
        excluded.add(parent)
        parent = table.get(parent, {}).get('parent')
    protected = {}
    selected = {}
    for pid, p in table.items():
        paths = list(references(p))
        if any(beneath(path, root) for path in paths for root in protected_roots(repo)):
            protected[pid] = p
            continue
        if pid in excluded:
            continue
        marker = p['env'].get(RUN_KEY)
        if p['env'].get(REPO_KEY) == str(repo) and marker in runs and (not run_id or marker == run_id):
            selected[pid] = (p, f'registered run {marker}', marker)
        elif not (run_id and not allowed):
            match = next((root for path in paths for root in allowed if beneath(path, root)), None)
            if match:
                selected[pid] = (p, f'fixture {match}', None)
    # Include only observed descendants of selected roots, preserving protected
    # subtrees. Detached/reparented children need the inherited marker or a path.
    for _ in range(len(table)):
        changed = False
        for pid, p in table.items():
            parent = p['parent']
            if parent in protected and pid not in protected:
                protected[pid] = p
                selected.pop(pid, None)
                changed = True
            elif parent in selected and pid not in selected and pid not in protected and pid not in excluded:
                selected[pid] = (p, f'descendant of {parent}', selected[parent][2])
                changed = True
        if not changed:
            break
    for pid in excluded | protected.keys():
        selected.pop(pid, None)
    return selected, protected


def summary(rows):
    return [{'pid': p['pid'], 'executable': Path(p['executable']).name, 'started': p['born'],
             'reason': reason, 'run': run} for p, reason, run in sorted(rows.values(), key=lambda x: x[0]['pid'])]


def stop(probe, repo, roots, run_id, timeout, force):
    events = []
    captured = {}

    def survivors():
        found, protected = discover(probe, repo, roots, run_id)
        captured.update(found)
        # Keep observed descendants after their parent exits, even if they scrub
        # their environment. Never transfer that authority to a reused PID.
        for pid, (observed, _, _) in list(captured.items()):
            current = probe.read(pid)
            if pid in protected or not current or current['identity'] != observed['identity']:
                captured.pop(pid)
        return dict(captured)

    for sig in ([signal.SIGTERM, signal.SIGKILL] if force else [signal.SIGTERM]):
        deadline = time.monotonic() + timeout
        sent = set()
        while True:
            candidates = survivors()
            for pid, (observed, _, _) in candidates.items():
                if observed['identity'] in sent:
                    continue
                # Never signal a process group or trust a PID saved in a report.
                current = probe.read(pid)
                if not current or current['identity'] != observed['identity']:
                    continue
                try:
                    os.kill(pid, sig)
                    events.append({'pid': pid, 'signal': sig.name})
                    sent.add(observed['identity'])
                except ProcessLookupError:
                    pass
            candidates = survivors()
            if not candidates or time.monotonic() >= deadline:
                break
            time.sleep(.1)
        if not candidates:
            break
    return events, candidates


def run_command(args, repo):
    command = args.command
    if command[:1] == ['--']:
        command = command[1:]
    if not command:
        raise ValueError('Supply a test command after --.')
    roots = [fixture_root(p, repo) for p in args.root]
    ident = str(uuid.uuid4())
    record = {'id': ident, 'repo': str(repo), 'label': args.label or Path(command[0]).name,
              'created': time.time(), 'roots': [str(p) for p in roots]}
    path = registry(repo, create=True) / (ident + '.json')
    with open(path, 'x', opener=lambda p, flags: os.open(p, flags, 0o600)) as out:
        json.dump(record, out)
    env = dict(os.environ, **{RUN_KEY: ident, REPO_KEY: str(repo)})
    print(f'Test run {ident} ({record["label"]})', file=sys.stderr, flush=True)
    child = subprocess.Popen(command, env=env, start_new_session=True)
    def forward(sig, _frame):
        if child.poll() is None:
            child.send_signal(sig)
    old = {sig: signal.signal(sig, forward) for sig in (signal.SIGTERM, signal.SIGINT)}
    try:
        code = child.wait()
    finally:
        for sig, handler in old.items():
            signal.signal(sig, handler)
    return code if code >= 0 else 128 - code


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='operation', required=True)
    for name in ('list', 'stop', 'run'):
        p = sub.add_parser(name)
        p.add_argument('--root', action='append', default=[], help='Additional disposable fixture directory; repeatable.')
        if name == 'run':
            p.add_argument('--label')
            p.add_argument('command', nargs=argparse.REMAINDER)
        else:
            p.add_argument('--run', dest='run_id', help='Select one recorded test run.')
            p.add_argument('--json', action='store_true')
        if name == 'stop':
            p.add_argument('--timeout', type=float, default=5, help='Seconds per TERM/KILL phase (default 5).')
            p.add_argument('--force', action='store_true', help='After TERM wait, SIGKILL only reverified test processes.')
    args = parser.parse_args()
    try:
        if args.operation == 'run':
            return run_command(args, REPO)
        if args.operation == 'stop' and not .1 <= args.timeout <= 30:
            raise ValueError('--timeout must be between 0.1 and 30 seconds.')
        roots = [fixture_root(p, REPO) for p in args.root]
        probe = Processes()
        found, protected = discover(probe, REPO, roots, args.run_id)
        report = {'candidates': summary(found), 'protected': [
            {'pid': p['pid'], 'executable': Path(p['executable']).name} for p in protected.values()]}
        if args.operation == 'stop':
            report['signals'], remaining = stop(probe, REPO, roots, args.run_id, args.timeout, args.force)
            report['remaining'] = summary(remaining)
        if args.json:
            print(json.dumps(report, indent=2))
        else:
            for p in report['candidates']:
                print(f'{p["pid"]:>7}  {p["executable"]}  {p["reason"]}')
            if not found:
                print('No identifiable test processes in the selected scope.')
            if args.operation == 'stop':
                print(f'Signals sent: {len(report["signals"])}; remaining: {len(report["remaining"])}.')
                if report['remaining'] and not args.force:
                    print('Use --force to escalate verified survivors to SIGKILL.')
            print(f'Protected normal app/pilot processes: {len(protected)}. No files or containers removed.')
        return 1 if report.get('remaining') else 0
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f'Test cleanup refused: {error}', file=sys.stderr)
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
