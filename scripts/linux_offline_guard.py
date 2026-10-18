"""Fixture-only network namespace with allowlisted host-loopback relays.

No IP route leaves the namespace. Unix relays carry only allocated fixture ports.
The mount namespace exposes system libraries, selected binaries and fixture state.
"""
import json
import os
from pathlib import Path
import select
import signal
import socket
import subprocess
import sys
import threading


def pump(a, b):
    try:
        while True:
            ready, _, _ = select.select([a, b], [], [], 30)
            if not ready:
                continue
            for source in ready:
                data = source.recv(65536)
                if not data:
                    return
                (b if source is a else a).sendall(data)
    finally:
        a.close(); b.close()


def serve(listener, connect):
    def client(a):
        try:
            b = connect(a)
            pump(a, b)
        except (OSError, ValueError):
            a.close()
    while True:
        try:
            a, _ = listener.accept()
        except OSError:
            return
        threading.Thread(target=client, args=(a,), daemon=True).start()


class LinuxGuard:
    def __init__(self, root, ports, readonly):
        self.root = Path(root)
        self.ports = list(dict.fromkeys(ports))
        # AF_UNIX paths have a short bound even when fixture paths are long.
        import tempfile
        self.relay_root = Path(tempfile.mkdtemp(prefix='cr-relay-'))
        self.path = self.relay_root / 'out.sock'
        self.listener = socket.socket(socket.AF_UNIX)
        self.listener.bind(str(self.path)); self.listener.listen()
        def connect(a):
            a.settimeout(3)
            port = b''
            while not port.endswith(b'\n') and len(port) < 8:
                byte = a.recv(1)
                if not byte: raise ValueError('Incomplete relay header')
                port += byte
            if not port.endswith(b'\n'): raise ValueError('Oversized relay header')
            target = int(port)
            a.settimeout(None)
            if target not in self.ports:
                raise PermissionError('Port not admitted')
            return socket.create_connection(('127.0.0.1', target), 5)
        threading.Thread(target=serve, args=(self.listener, connect), daemon=True).start()
        self.readonly = readonly
        self.inbound = []

    def argv(self, command):
        command = list(map(str, command))
        inbound = None
        if '--no-reuse-running' in command:
            port = int(command[command.index('--port') + 1])
            path = self.relay_root / f'in-{port}.sock'
            listener = socket.socket(); listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            listener.bind(('127.0.0.1', port)); listener.listen()
            def connect(_, target=path):
                sock = socket.socket(socket.AF_UNIX)
                try: sock.connect(str(target)); return sock
                except OSError: sock.close(); raise
            threading.Thread(target=serve, args=(listener, connect), daemon=True).start()
            self.inbound.append(listener)
            inbound = [port, str(path)]
        args = ['/usr/bin/bwrap', '--unshare-user', '--unshare-net', '--unshare-pid', '--die-with-parent']
        # No user home, project configuration or credentials are exposed.
        for path in ['/usr', '/bin', '/lib', '/lib64', '/etc/ld.so.cache', '/etc/ssl']:
            if Path(path).exists():
                args += ['--ro-bind', path, path]
        args += ['--proc', '/proc', '--dev', '/dev', '--tmpfs', '/tmp']
        for path in self.readonly:
            args += ['--ro-bind', str(path), str(path)]
        args += ['--bind', str(self.root), str(self.root), '--bind', str(self.relay_root), str(self.relay_root),
                 '--chdir', str(self.root), '/usr/bin/python3', str(Path(__file__).resolve()),
                 str(self.path), json.dumps(self.ports), json.dumps(inbound), *command]
        return ['/usr/bin/python3', str(Path(__file__).resolve()), '--launch', *args]

    def close(self):
        self.listener.close()
        for sock in self.inbound:
            sock.close()
        for path in self.relay_root.iterdir():
            path.unlink()
        self.relay_root.rmdir()


def inside():
    relay, ports, inbound, *command = sys.argv[1:]
    ports, inbound = json.loads(ports), json.loads(inbound)
    # bubblewrap configures loopback; the namespace has no external interface.
    listeners = []
    for port in ports:
        if inbound and port == inbound[0]:
            continue
        sock = socket.socket(); sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        sock.bind(('127.0.0.1', port)); sock.listen(); listeners.append((sock, port))
    if inbound:
        sock = socket.socket(socket.AF_UNIX); sock.bind(inbound[1]); sock.listen(); listeners.append((sock, None))
    parent = os.getpid()
    if os.fork() == 0:
        import ctypes
        ctypes.CDLL(None).prctl(1, signal.SIGTERM)
        if os.getppid() != parent:
            os._exit(0)
        for sock, port in listeners:
            def connect(_, port=port):
                if port is None:
                    return socket.create_connection(('127.0.0.1', inbound[0]), 5)
                out = socket.socket(socket.AF_UNIX); out.connect(relay); out.sendall(f'{port}\n'.encode()); return out
            threading.Thread(target=serve, args=(sock, connect), daemon=True).start()
        signal.pause(); os._exit(0)
    for sock, _ in listeners:
        sock.close()
    os.execvpe(command[0], command, os.environ)


if __name__ == '__main__':
    if sys.argv[1] == '--launch':
        command = sys.argv[2:]
        for name in os.listdir('/proc/self/fd'):
            try:
                target = os.readlink('/proc/self/fd/' + name)
                if target.endswith('/omp-process/lease.lock'):
                    command[1:1] = ['--sync-fd', name]
            except OSError:
                pass
        # Keep the owned leader visible to host lsof across the user namespace.
        raise SystemExit(subprocess.call(command, close_fds=False))
    inside()
