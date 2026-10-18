"""Behavior checks for the Linux-only disposable fixture boundary."""
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from linux_offline_guard import LinuxGuard, serve
from mailbox_contract_fixture import free_port


@unittest.skipUnless(sys.platform == 'linux', 'Linux fixture boundary')
class GuardTests(unittest.TestCase):
    def test_network_and_filesystem_confinement_and_closed_headers(self):
        with tempfile.TemporaryDirectory(prefix='cr-guard-test-') as directory:
            root = Path(directory)
            target = socket.socket(); target.bind(('127.0.0.1', 0)); target.listen()
            port = target.getsockname()[1]
            guard = LinuxGuard(root, [port], [Path(__file__).with_name('linux_offline_guard.py').resolve()])
            before = threading.active_count()
            try:
                for header in [b'', b'bad\n', b'12345678']:
                    sock = socket.socket(socket.AF_UNIX); sock.connect(str(guard.path)); sock.sendall(header); sock.close()
                time.sleep(.1)
                self.assertLessEqual(threading.active_count(), before)
                code = f'''import os,socket
s=socket.create_connection(('127.0.0.1',{port}),2);s.close()
for host,port in [('192.0.2.1',443),('127.0.0.1',1)]:
 try:socket.create_connection((host,port),1);raise AssertionError('network escaped')
 except OSError:pass
assert not os.path.exists('/proc/{os.getpid()}/root')
assert not os.path.exists({str(Path.home() / '.codex/auth.json')!r})
'''
                subprocess.run(guard.argv(['/usr/bin/python3', '-c', code]), check=True, timeout=10)
            finally:
                guard.close(); target.close()

    def test_inbound_fixture_listener_survives_readonly_mount_iteration(self):
        with tempfile.TemporaryDirectory(prefix='cr-guard-test-') as directory:
            root = Path(directory); port = free_port()
            guard = LinuxGuard(root, [port], [Path(__file__).with_name('linux_offline_guard.py').resolve()])
            code = f"import socket;s=socket.socket();s.bind(('127.0.0.1',{port}));s.listen();c,_=s.accept();c.recv(10);c.sendall(b'OK');c.close()"
            proc = subprocess.Popen(guard.argv(['/usr/bin/python3', '-c', code, '--no-reuse-running', '--port', str(port)]))
            try:
                deadline = time.monotonic() + 5
                while not (guard.relay_root / f'in-{port}.sock').exists():
                    if time.monotonic() > deadline: self.fail('Listener did not start')
                    time.sleep(.01)
                time.sleep(.1)
                with socket.create_connection(('127.0.0.1', port), 2) as sock:
                    sock.sendall(b'hello'); self.assertEqual(sock.recv(2), b'OK')
                self.assertEqual(proc.wait(5), 0)
            finally:
                if proc.poll() is None: proc.kill(); proc.wait()
                guard.close()


if __name__ == '__main__':
    unittest.main()
