#!/usr/bin/env python3
"""OS boundary: inherited process lock and epoch-fenced one-shot checked exec."""
import fcntl
import hashlib
import json
import os
from pathlib import Path
import selectors
import subprocess
import sys

operation, directory = sys.argv[1:3]
root = Path(directory)
if operation == 'checked-exec':
    def checked_error(error_type, error, _traceback):
        os.write(2, f'Checked process helper failed: {error_type.__name__}: {error}\n'.encode()[:512])
    sys.excepthook = checked_error
fd = os.open(root / 'lease.lock', os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
if operation == 'probe':
    try:
        fcntl.flock(fd, fcntl.LOCK_SH | fcntl.LOCK_NB)
        print('free')
    except BlockingIOError:
        print('held')
    sys.exit(0)

def durable_write(path, value):
    with path.open('x') as output:
        os.chmod(path, 0o600)
        output.write(json.dumps(value, separators=(',', ':')))
        output.flush()
        os.fsync(output.fileno())
    directory_fd = os.open(root, os.O_RDONLY)
    try:
        os.fsync(directory_fd)
    finally:
        os.close(directory_fd)

def publish_owner(value):
    """`owner.json` is either absent or complete, as in the native helper: an inspector that sees the
    receipt appear never reads a torn one, and a helper killed while writing leaves only the partial file."""
    partial = root / 'owner.json.partial'
    durable_write(partial, value)
    # A hard link fails if a receipt already exists, unlike rename.
    os.link(partial, root / 'owner.json')
    partial.unlink()
    directory_fd = os.open(root, os.O_RDONLY)
    try:
        os.fsync(directory_fd)
    finally:
        os.close(directory_fd)

if operation == 'hold':
    authority = os.open(root / 'dispatch.lock', os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    try:
        fcntl.flock(authority, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        # A checked helper can briefly hold dispatch SH after its former controller
        # has exited. It must finish its epoch/lease check before any takeover.
        try:
            fcntl.flock(fd, fcntl.LOCK_SH | fcntl.LOCK_NB)
        except BlockingIOError:
            print('controller-held', flush=True)
        else:
            print('dispatch-busy', flush=True)
        sys.exit(1)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        print('controller-held', flush=True)
        sys.exit(1)
    epoch = sys.argv[3]
    temporary = root / ('epoch.' + epoch)
    durable_write(temporary, {'epoch': epoch})
    os.replace(temporary, root / 'epoch.json')
    directory_fd = os.open(root, os.O_RDONLY)
    try:
        os.fsync(directory_fd)
    finally:
        os.close(directory_fd)
    fcntl.flock(authority, fcntl.LOCK_SH)
    print('held', flush=True)
    sys.stdin.read()
    sys.exit(0)

if operation == 'checked-exec':
    token, spec_digest, epoch, controller = sys.argv[3:7]
    controller_root = Path(controller)
    authority = os.open(controller_root / 'dispatch.lock', os.O_RDONLY | os.O_NOFOLLOW)
    fcntl.flock(authority, fcntl.LOCK_SH)
    if json.loads((controller_root / 'epoch.json').read_text()) != {'epoch': epoch}:
        sys.exit('Controller epoch no longer owns checked dispatch.')
    controller_fd = os.open(controller_root / 'lease.lock', os.O_RDONLY | os.O_NOFOLLOW)
    try:
        fcntl.flock(controller_fd, fcntl.LOCK_SH | fcntl.LOCK_NB)
        sys.exit('Controller lock is no longer held.')
    except BlockingIOError:
        pass
    finally:
        os.close(controller_fd)

if operation == 'checked-exec':
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        sys.exit('Process lease already held.')
    # The lease is one-shot: a helper delayed until the winner has exited acquires the
    # free lock afterwards, so the durable owner record, not the lock, proves prior use.
    if (root / 'owner.json').exists() or (root / 'owner.json.partial').exists():
        sys.exit('Process lease already held.')
else:
    fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
if operation in ('checked-exec', 'seal'):
    token, spec_digest = sys.argv[3:5]
    if operation == 'seal' and not (root / 'launch.json').exists():
        # Admission was durable, but preparation never finished. Lock the one-shot inode
        # before sealing: a delayed helper that reaches launch later cannot execute.
        if set(os.listdir(root)) - {'lease.lock', 'seal.json'}:
            sys.exit('Unprepared checked root contains other process artifacts.')
        expected = {'token': token, 'specSha256': spec_digest}
        if (root / 'seal.json').exists():
            if json.loads((root / 'seal.json').read_text()) != expected:
                sys.exit('Checked process seal mismatch.')
        else:
            durable_write(root / 'seal.json', expected)
        sys.exit(0)
    raw = (root / 'launch.json').read_bytes()
    config = json.loads(raw)
    if (config.get('version') != 1 or config.get('token') != token or
            hashlib.sha256(raw).hexdigest() != spec_digest):
        sys.exit('Checked process identity mismatch.')
    if operation == 'checked-exec':
        if config.get('epoch') != epoch or config.get('controllerRoot') != controller:
            sys.exit('Checked process authority mismatch.')
        if (root / 'seal.json').exists():
            sys.exit('Checked process was sealed.')
    else:
        if not (root / 'seal.json').exists():
            durable_write(root / 'seal.json', {'token': token, 'specSha256': spec_digest})
        elif json.loads((root / 'seal.json').read_text()) != {'token': token, 'specSha256': spec_digest}:
            sys.exit('Checked process seal mismatch.')
        sys.exit(0)
else:
    config = json.loads((root / 'launch.json').read_text())

os.set_inheritable(fd, True)
started = subprocess.check_output([os.environ.get('CREWROOM_PROCESS_INSPECTOR', '/bin/ps'), '-o', 'lstart=', '-p', str(os.getpid())], text=True).strip()
owner = {'pid': os.getpid(), 'pgid': os.getpgrp(), 'started': started, 'token': config['token']}
if operation == 'checked-exec':
    owner.update({'specSha256': spec_digest, 'epoch': epoch})
publish_owner(owner)
if operation == 'checked-exec':
    # Only the checked helper opens retained logs. Refused helpers report through their
    # separate stderr pipe and can never append to the winning command's output.
    cap = 128 * 1024
    logs = {name: os.open(root / (name + '.log'), os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
            for name in ('stdout', 'stderr')}
    capture = {name: {'retainedBytes': 0, 'truncated': False} for name in logs}
    child = subprocess.Popen([config['command'], *config['args']], cwd=config['cwd'], env=config['env'],
                             stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                             close_fds=True, pass_fds=(fd,))
    # The child keeps only the operation lease. A new controller can acquire the
    # dispatch lock while this leader pumps a still-running checked command.
    fcntl.flock(authority, fcntl.LOCK_UN)
    os.close(authority)
    selector = selectors.DefaultSelector()
    selector.register(child.stdout, selectors.EVENT_READ, 'stdout')
    selector.register(child.stderr, selectors.EVENT_READ, 'stderr')
    failed_streams = set()
    while selector.get_map():
        for key, _ in selector.select():
            chunk = os.read(key.fileobj.fileno(), 65536)
            if not chunk:
                selector.unregister(key.fileobj)
                key.fileobj.close()
                continue
            info = capture[key.data]
            remaining = cap - info['retainedBytes']
            if len(chunk) > remaining:
                info['truncated'] = True
            if remaining and key.data not in failed_streams:
                prefix = memoryview(chunk)[:remaining]
                try:
                    while prefix:
                        written = os.write(logs[key.data], prefix)
                        if not written:
                            raise OSError('No progress writing retained output.')
                        info['retainedBytes'] += written
                        prefix = prefix[written:]
                except OSError:
                    failed_streams.add(key.data)  # Still drain both pipes so Git cannot deadlock.
    selector.close()
    for log in logs.values():
        os.close(log)
    result = child.wait()
    durable_write(root / 'capture.json', capture)
    if failed_streams:
        sys.exit('Checked output capture failed; command outcome requires reconciliation.')
    if result < 0:
        os.kill(os.getpid(), -result)  # Preserve signal exit semantics for the live Node parent.
    sys.exit(result)
os.chdir(config['cwd'])
os.execvpe(config['command'], [config['command'], *config['args']], config['env'])
