#!/usr/bin/env python3
"""Run tests with the system's resource limits when the caller inherited a hard RLIMIT_RTTIME of 0 (FOR-68).

On Linux, RLIMIT_RTTIME bounds the CPU time a real-time thread may use without blocking. With a hard
limit of 0, the kernel's CPU-timer check sends SIGKILL ("At the hard limit, send SIGKILL") the first
time it evaluates a thread, so processes die mid-run with no failing test and no sender. An
unprivileged process cannot raise a hard limit, so the only remedy is to start the command from a
parent with the default limit. This script starts it as a transient `systemd --user` service, which
gets systemd's defaults (RTTIME unlimited), and waits for it with stdio attached.

    python3 scripts/system_limits.py check          # report the limit; exit 1 when it would SIGKILL tests
    python3 scripts/system_limits.py run -- cargo test --workspace --locked
    python3 scripts/system_limits.py probe          # arm a thread CPU timer here; dies with SIGKILL under a 0 limit

`run` executes the command directly when the inherited limits are safe. Otherwise it passes the
working directory, arguments and environment through a private 0600 file that the service consumes
and deletes before it executes the command, so no environment value appears on a command line.
"""
import argparse
import ctypes
import json
import os
from pathlib import Path
import resource
import shutil
import signal
import subprocess
import sys
import tempfile
import time

STEP = 'python3 scripts/system_limits.py'
SCRIPT = Path(__file__).resolve()
RLIMIT_RTTIME = getattr(resource, 'RLIMIT_RTTIME', None)
# Values systemd sets for its own process; a forwarded copy would misdescribe the new service.
SERVICE_OWNED = {'INVOCATION_ID', 'JOURNAL_STREAM', 'MANAGERPID', 'MANAGERPIDFDID', 'NOTIFY_SOCKET', 'SYSTEMD_EXEC_PID',
                 'MEMORY_PRESSURE_WATCH', 'MEMORY_PRESSURE_WRITE', 'LISTEN_FDS', 'LISTEN_PID', 'LISTEN_FDNAMES'}


def rttime_limits(getrlimit=resource.getrlimit):
    """(soft, hard) for RLIMIT_RTTIME, or None where the platform has no such limit."""
    if RLIMIT_RTTIME is None:
        return None
    return getrlimit(RLIMIT_RTTIME)


def hazard(limits):
    """A one-line explanation when these RTTIME limits make the kernel kill CPU-timer users, else None."""
    if limits is None:
        return None
    soft, hard = limits
    if hard == 0:
        return ('RLIMIT_RTTIME hard limit is 0: the kernel sends SIGKILL to any thread whose CPU-timer check runs, '
                'and this process cannot raise it. Start tests through '
                f'`{STEP} run -- <command>`, or from a shell that does not inherit this limit.')
    if soft == 0:
        return f'RLIMIT_RTTIME soft limit is 0 (hard {hard}); raise it with `ulimit -r` or use `{STEP} run`.'
    return None


def describe(limits):
    if limits is None:
        return 'RLIMIT_RTTIME: not applicable on this platform.'
    shown = ['unlimited' if value == resource.RLIM_INFINITY else f'{value} us' for value in limits]
    return f'RLIMIT_RTTIME: soft {shown[0]}, hard {shown[1]}.'


def raise_soft(limits, setrlimit=resource.setrlimit):
    """Lift a zero soft limit up to a nonzero hard limit; returns the limits now in force."""
    soft, hard = limits
    if soft == 0 and hard != 0:
        setrlimit(RLIMIT_RTTIME, (hard, hard))
        return hard, hard
    return limits


def forwarded_environment(environ):
    return {key: value for key, value in environ.items() if key not in SERVICE_OWNED}


def write_launch(directory, argv, cwd, environ):
    """Write the private launch file the service consumes; returns its path."""
    descriptor, path = tempfile.mkstemp(prefix='crewroom-system-limits-', suffix='.json', dir=directory)
    with os.fdopen(descriptor, 'w') as handle:
        os.fchmod(handle.fileno(), 0o600)
        json.dump({'argv': list(argv), 'cwd': str(cwd), 'env': forwarded_environment(environ)}, handle)
    return Path(path)


def read_launch(path):
    """Load and delete a launch file; refuses one that other users could have read or replaced."""
    path = Path(path)
    with open(path, 'rb') as handle:
        status = os.fstat(handle.fileno())
        if status.st_uid != os.getuid() or status.st_mode & 0o077:
            raise PermissionError(f'{path} must be private to this user.')
        launch = json.loads(handle.read())
    path.unlink()
    return launch


def service_command(launch, systemd_run, python=sys.executable):
    """The transient-service invocation. Only paths appear here; values stay in the private file."""
    unit = f'crewroom-system-limits-{os.getpid()}-{time.monotonic_ns()}'
    return [systemd_run, '--user', '--wait', '--pipe', '--collect', '--quiet', f'--unit={unit}',
            f'--working-directory={Path.cwd()}', python, '-I', str(SCRIPT), 'exec-launch', str(launch)]


def exit_status(code):
    """Shell-style status: a signal death becomes 128 + signal, as the caller's shell would report."""
    return 128 - code if code < 0 else code


def run(argv, environ=None, getrlimit=resource.getrlimit, setrlimit=resource.setrlimit,
        which=shutil.which, spawn=subprocess.call, out=sys.stderr):
    environ = dict(os.environ if environ is None else environ)
    limits = rttime_limits(getrlimit)
    if limits is not None and hazard(limits):
        try:
            limits = raise_soft(limits, setrlimit)
        except (OSError, ValueError):
            pass
    if limits is None or not hazard(limits):
        return exit_status(spawn(argv, env=environ))
    systemd_run = which('systemd-run')
    if systemd_run is None:
        print(f'{hazard(limits)} systemd-run is unavailable here, so the command was not started.', file=out)
        return 2
    launch = write_launch(Path(environ.get('XDG_RUNTIME_DIR') or tempfile.gettempdir()), argv, Path.cwd(), environ)
    try:
        return exit_status(spawn(service_command(launch, systemd_run)))
    finally:
        launch.unlink(missing_ok=True)


def exec_launch(path):
    launch = read_launch(path)
    os.chdir(launch['cwd'])
    # Preserve the command's own exit status and signal; this process is replaced.
    os.execvpe(launch['argv'][0], launch['argv'], launch['env'])


def probe(milliseconds=20):
    """Arm a per-thread CPU-time timer and burn CPU past it. Survives (exit 0) with a safe limit."""
    libc = ctypes.CDLL(None, use_errno=True)
    fired = []
    signal.signal(signal.SIGALRM, lambda *_: fired.append(True))

    class Timespec(ctypes.Structure):
        _fields_ = [('tv_sec', ctypes.c_long), ('tv_nsec', ctypes.c_long)]

    class Itimerspec(ctypes.Structure):
        _fields_ = [('it_interval', Timespec), ('it_value', Timespec)]

    timer = ctypes.c_void_p()
    clock_thread_cputime_id = 3
    if libc.timer_create(clock_thread_cputime_id, None, ctypes.byref(timer)) != 0:
        raise OSError(ctypes.get_errno(), 'timer_create failed')
    spec = Itimerspec(Timespec(0, 0), Timespec(0, milliseconds * 1_000_000))
    if libc.timer_settime(timer, 0, ctypes.byref(spec), None) != 0:
        raise OSError(ctypes.get_errno(), 'timer_settime failed')
    deadline = time.monotonic() + 1.0
    while not fired and time.monotonic() < deadline:
        pass
    print('thread CPU timer fired; this process was not killed' if fired else 'thread CPU timer did not fire')
    return 0 if fired else 1


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest='command', required=True)
    commands.add_parser('check', help='report RLIMIT_RTTIME; exit 1 when it would SIGKILL tests')
    run_parser = commands.add_parser('run', help='run a command with system limits')
    run_parser.add_argument('argv', nargs=argparse.REMAINDER)
    commands.add_parser('probe', help='arm a thread CPU timer in this process')
    launch_parser = commands.add_parser('exec-launch', help=argparse.SUPPRESS)
    launch_parser.add_argument('path')
    args = parser.parse_args(argv)
    if args.command == 'check':
        limits = rttime_limits()
        print(describe(limits))
        problem = hazard(limits)
        if problem:
            print(problem, file=sys.stderr)
            return 1
        return 0
    if args.command == 'run':
        command = args.argv[1:] if args.argv[:1] == ['--'] else args.argv
        if not command:
            parser.error('run needs a command after --')
        return run(command)
    if args.command == 'probe':
        return probe()
    exec_launch(args.path)
    return 0


if __name__ == '__main__':
    sys.exit(main())
