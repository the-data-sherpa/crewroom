#!/usr/bin/env python3
"""FOR-8 native Claude auth probes with fake credentials and isolated networking.

Linux only. The script re-execs inside a new user/network namespace, exposes only
loopback, and answers HTTPS through a local TLS fixture. It never reads native
operator credentials or contacts a real authentication/inference endpoint.
"""

import argparse
import ctypes
from datetime import datetime, timezone
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import signal
import socket
import ssl
import subprocess
import sys
import threading
import time
import uuid


SCOPES = "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload"
FAKE_ACCESS = "sk-ant-oat01-crewroom-synthetic-expired"
FAKE_REFRESH = "sk-ant-ort01-crewroom-synthetic-refresh"
ACCOUNT = "11111111-1111-4111-8111-111111111111"
ORGANIZATION = "22222222-2222-4222-8222-222222222222"


def save(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")
    path.chmod(0o600)


def eventually(predicate, seconds=20):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(0.05)
    return bool(predicate())


class Fixture:
    def __init__(self, root):
        self.events = []
        self.refresh_started = threading.Event()
        self.release_refresh = threading.Event()
        self.release_refresh.set()
        self.reject_refresh = False
        self.active_refreshes = 0
        self.max_active_refreshes = 0
        self.rotations = 0
        self.current_refresh = FAKE_REFRESH
        self.guard = threading.Lock()
        cert = root / "fixture.crt"
        key = root / "fixture.key"
        subprocess.run([
            "openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
            "-keyout", str(key), "-out", str(cert), "-days", "1", "-subj",
            "/CN=api.anthropic.com", "-addext",
            "subjectAltName=DNS:api.anthropic.com,DNS:platform.claude.com,DNS:claude.ai,DNS:claude.com",
        ], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        key.chmod(0o600)
        self.cert = cert
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(cert, key)
        owner = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *_):
                pass

            def do_CONNECT(self):
                host, _, port = self.path.rpartition(":")
                if host not in {"api.anthropic.com", "platform.claude.com", "claude.ai", "claude.com"} or port != "443":
                    self.answer(502, {"error": "fixture refuses this destination"})
                    return
                self.send_response(200, "Connection established")
                self.end_headers()
                try:
                    self.connection = context.wrap_socket(self.connection, server_side=True)
                    self.rfile = self.connection.makefile("rb", self.rbufsize)
                    self.wfile = self.connection.makefile("wb", self.wbufsize)
                    self.close_connection = False
                    self.handle_one_request()
                except (OSError, ssl.SSLError):
                    self.close_connection = True

            def answer(self, status, body, content_type="application/json"):
                data = body if isinstance(body, bytes) else json.dumps(body).encode()
                try:
                    self.send_response(status)
                    self.send_header("Content-Type", content_type)
                    self.send_header("Content-Length", str(len(data)))
                    self.end_headers()
                    self.wfile.write(data)
                    self.wfile.flush()
                except (OSError, ssl.SSLError):
                    pass

            def do_GET(self):
                owner.events.append({"kind": "get", "path": self.path.split("?")[0]})
                self.answer(404, {"error": "fixture has no such resource"})

            def do_POST(self):
                raw = self.rfile.read(int(self.headers.get("Content-Length", "0")))
                if self.headers.get("Content-Encoding") == "gzip":
                    import gzip
                    raw = gzip.decompress(raw)
                try:
                    body = json.loads(raw)
                except ValueError:
                    body = {}
                path = self.path.split("?")[0]
                if path == "/v1/oauth/token":
                    with owner.guard:
                        owner.active_refreshes += 1
                        owner.max_active_refreshes = max(owner.max_active_refreshes, owner.active_refreshes)
                        owner.events.append({"kind": "refresh_start", "at": time.monotonic(),
                                             "fakeRefreshMatched": body.get("refresh_token") == FAKE_REFRESH})
                    owner.refresh_started.set()
                    owner.release_refresh.wait(90)
                    disconnected = False
                    previous_timeout = self.connection.gettimeout()
                    try:
                        self.connection.settimeout(0.02)
                        disconnected = self.connection.recv(1) == b""
                    except (TimeoutError, ssl.SSLWantReadError):
                        pass
                    except (OSError, ssl.SSLError):
                        disconnected = True
                    finally:
                        self.connection.settimeout(previous_timeout)
                    with owner.guard:
                        owner.active_refreshes -= 1
                        owner.events.append({"kind": "refresh_end", "at": time.monotonic()})
                        rejected = owner.reject_refresh or body.get("refresh_token") != owner.current_refresh
                        if disconnected:
                            owner.events.append({"kind": "refresh_abandoned_before_rotation"})
                        elif not rejected:
                            owner.rotations += 1
                            owner.current_refresh = "sk-ant-ort01-crewroom-synthetic-rotated"
                    if disconnected:
                        self.close_connection = True
                        return
                    if rejected:
                        self.answer(400, {"error": "invalid_grant", "error_description": "synthetic expired login"})
                    else:
                        self.answer(200, {"access_token": "sk-ant-oat01-crewroom-synthetic-fresh",
                                          "refresh_token": "sk-ant-ort01-crewroom-synthetic-rotated",
                                          "expires_in": 3600, "refresh_token_expires_in": 86400,
                                          "scope": SCOPES, "token_type": "Bearer",
                                          "account": {"uuid": ACCOUNT, "email_address": "fixture@example.invalid"},
                                          "organization": {"uuid": ORGANIZATION}})
                    return
                if path == "/v1/messages/count_tokens":
                    self.answer(200, {"input_tokens": 10})
                    return
                if path == "/v1/messages":
                    owner.events.append({"kind": "message", "at": time.monotonic(),
                                         "freshTokenUsed": self.headers.get("Authorization") == "Bearer sk-ant-oat01-crewroom-synthetic-fresh"})
                    ident = "msg_fixture_" + uuid.uuid4().hex
                    events = [
                        {"type": "message_start", "message": {"id": ident, "type": "message", "role": "assistant",
                         "model": body.get("model", "sonnet"), "content": [], "stop_reason": None, "stop_sequence": None,
                         "usage": {"input_tokens": 10, "output_tokens": 0}}},
                        {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}},
                        {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "FIXTURE_OK"}},
                        {"type": "content_block_stop", "index": 0},
                        {"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence": None}, "usage": {"output_tokens": 4}},
                        {"type": "message_stop"},
                    ]
                    data = "".join("event: " + e["type"] + "\ndata: " + json.dumps(e) + "\n\n" for e in events).encode()
                    self.answer(200, data, "text/event-stream")
                    return
                owner.events.append({"kind": "post", "path": path})
                self.answer(404, {"error": "fixture has no such resource"})

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def close(self):
        self.release_refresh.set()
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=2)


def fake_home(root, credentials=True):
    home = root / "home"
    config = home / ".claude"
    config.mkdir(parents=True, mode=0o700)
    home.chmod(0o700)
    save(home / ".claude.json", {"hasCompletedOnboarding": True, "oauthAccount": {
        "accountUuid": ACCOUNT, "organizationUuid": ORGANIZATION,
        "emailAddress": "fixture@example.invalid", "billingType": "stripe_subscription",
        "accountCreatedAt": "2020-01-01T00:00:00Z", "subscriptionCreatedAt": "2020-01-01T00:00:00Z",
        "ccOnboardingFlags": {},
    }})
    if credentials:
        save(config / ".credentials.json", {"claudeAiOauth": {
            "accessToken": FAKE_ACCESS, "refreshToken": FAKE_REFRESH,
            "expiresAt": int(time.time() * 1000) - 60000,
            "refreshTokenExpiresAt": int(time.time() * 1000) + 86400000,
            "scopes": SCOPES.split(), "subscriptionType": "team", "rateLimitTier": "default_claude_max_5x",
        }})
    return home


class Child:
    def __init__(self, binary, home, root, fixture, intermediate_parent=False):
        root.mkdir(mode=0o700)
        self.root = root
        self.lines = []
        self.stderr = []
        proxy = "http://127.0.0.1:" + str(fixture.server.server_port)
        env = {"HOME": str(home), "PATH": "/usr/bin:/bin", "LANG": "C.UTF-8",
               "HTTPS_PROXY": proxy, "HTTP_PROXY": proxy, "ALL_PROXY": proxy,
               "NODE_EXTRA_CA_CERTS": str(fixture.cert), "SSL_CERT_FILE": str(fixture.cert),
               "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1", "DISABLE_AUTOUPDATER": "1",
               "CLAUDE_CODE_RESUME_INTERRUPTED_TURN": "0", "CLAUDE_CODE_MAX_RETRIES": "0"}
        argv = [str(binary), "--safe-mode", "-p", "--input-format", "stream-json", "--output-format", "stream-json",
                "--verbose", "--tools", "", "--strict-mcp-config", "--mcp-config", '{"mcpServers":{}}',
                "--setting-sources", "", "--disable-slash-commands", "--no-chrome", "--no-session-persistence",
                "--model", "sonnet", "--effort", "low", "--permission-mode", "dontAsk",
                "--system-prompt", "Reply FIXTURE_OK.", "--debug-file", str(root / "debug.log")]
        self.intermediate_parent = intermediate_parent
        self.parent_killed = False
        self.native_exit_code = None
        if intermediate_parent:
            # This launcher deliberately does not request parent-death cleanup.
            # Killing it tests the native CLI's own behavior, not Crewroom's.
            launcher = ("import subprocess,sys; from pathlib import Path; "
                        "p=subprocess.Popen(sys.argv[2:],start_new_session=True); "
                        "Path(sys.argv[1]).write_text(str(p.pid)); sys.exit(p.wait())")
            argv = [sys.executable, "-c", launcher, str(root / "native.pid"), *argv]
        self.process = subprocess.Popen(argv, cwd=root, env=env, stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
        self.native_pid = self.process.pid
        if intermediate_parent:
            if not eventually(lambda: (root / "native.pid").exists(), 5):
                self.process.kill()
                self.process.wait()
                raise RuntimeError("native child did not launch")
            self.native_pid = int((root / "native.pid").read_text())
        def reader(stream, target):
            for line in stream:
                target.append(line)
        self.readers = [threading.Thread(target=reader, args=(self.process.stdout, self.lines), daemon=True),
                        threading.Thread(target=reader, args=(self.process.stderr, self.stderr), daemon=True)]
        for thread in self.readers:
            thread.start()
        for frame in [
            {"type": "control_request", "request_id": "initialize", "request": {"subtype": "initialize", "hooks": None}},
            {"type": "user", "uuid": str(uuid.uuid4()), "session_id": "default", "parent_tool_use_id": None,
             "client_composed": True, "message": {"role": "user", "content": "Fixture authentication check."}},
        ]:
            self.process.stdin.write(json.dumps(frame) + "\n")
        self.process.stdin.close()

    def poll(self):
        if not self.parent_killed:
            return self.process.poll()
        if self.native_exit_code is None:
            try:
                pid, status = os.waitpid(self.native_pid, os.WNOHANG)
            except ChildProcessError:
                return self.native_exit_code
            if pid:
                self.native_exit_code = os.waitstatus_to_exitcode(status)
        return self.native_exit_code

    def kill_parent(self):
        if not self.intermediate_parent:
            raise RuntimeError("child has no intermediate parent")
        self.process.kill()
        self.process.wait(timeout=5)
        self.parent_killed = True

    def stop(self, hard=False):
        if self.poll() is None:
            try:
                os.killpg(self.native_pid, signal.SIGKILL if hard else signal.SIGTERM)
            except ProcessLookupError:
                pass
            if not eventually(lambda: self.poll() is not None, 5):
                try:
                    os.killpg(self.native_pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                eventually(lambda: self.poll() is not None, 5)
        if self.process.poll() is None:
            self.process.kill()
            self.process.wait(timeout=5)
        for thread in self.readers:
            thread.join(timeout=2)

    def result(self, seconds=30):
        finished = eventually(lambda: self.poll() is not None, seconds)
        if not finished:
            self.stop()
        for thread in self.readers:
            thread.join(timeout=2)
        frames = []
        for line in self.lines:
            try:
                frames.append(json.loads(line))
            except ValueError:
                pass
        results = [f for f in frames if f.get("type") == "result"]
        last = results[-1] if results else {}
        diagnostic = "\n".join(self.lines + self.stderr).lower()
        debug = (self.root / "debug.log").read_text() if (self.root / "debug.log").exists() else ""
        return {"finished": finished, "exitCode": self.poll(), "resultSeen": bool(results),
                "isError": last.get("is_error"), "subtype": last.get("subtype"),
                "fixtureReply": last.get("result") == "FIXTURE_OK",
                "loginErrorMentioned": any(t in diagnostic for t in ["not logged in", "login expired", "token expired", "oauth", "authentication"]),
                "lockContendedMentioned": "lock" in debug.lower() and "contend" in debug.lower()}


def run_case(name, binary, root):
    root.mkdir(mode=0o700)
    fixture = Fixture(root)
    home = fake_home(root, credentials=name != "missing")
    children = []
    details = {}
    try:
        if name == "expired":
            fixture.reject_refresh = True
        if name in {"concurrent", "owner_death", "parent_death"}:
            fixture.release_refresh.clear()
        first = Child(binary, home, root / "first", fixture, intermediate_parent=name == "parent_death")
        children.append(first)
        if name in {"concurrent", "owner_death", "parent_death"}:
            details["firstRefreshReached"] = fixture.refresh_started.wait(25)
            details["nativeLockPresent"] = (home / ".claude" / ".oauth_refresh.lock").is_dir()
            if name == "parent_death":
                first.kill_parent()
                details["launcherKilled"] = first.process.returncode == -signal.SIGKILL
                details["nativeOwnerSurvivedParent"] = first.poll() is None
            second = Child(binary, home, root / "second", fixture)
            children.append(second)
            time.sleep(2)
            details["refreshStartsWhileFirstBlocked"] = sum(e["kind"] == "refresh_start" for e in fixture.events)
            if name == "owner_death":
                first.stop(hard=True)
                details["killedOwnerReaped"] = first.process.poll() is not None
                # A dead owner's TLS request gets no usable response. Release
                # the fixture handler, then observe native stale-lock recovery.
                fixture.release_refresh.set()
                details["second"] = second.result(80)
                details["lockPresentAfterReplacement"] = (home / ".claude" / ".oauth_refresh.lock").exists()
            else:
                fixture.release_refresh.set()
                details["first"] = first.result()
                details["second"] = second.result()
        else:
            details["first"] = first.result()
        details["events"] = fixture.events
        details["maxSimultaneousRefreshes"] = fixture.max_active_refreshes
        details["refreshCount"] = sum(e["kind"] == "refresh_start" for e in fixture.events)
        details["successfulSyntheticRotations"] = fixture.rotations
        details["messageCount"] = sum(e["kind"] == "message" for e in fixture.events)
        messages = [e for e in fixture.events if e["kind"] == "message"]
        details["allMessagesUsedRotatedToken"] = all(e["freshTokenUsed"] for e in messages) if messages else None
        details["credentialMode"] = oct((home / ".claude" / ".credentials.json").stat().st_mode & 0o777) if name != "missing" else None
        checks = {}
        if name in {"missing", "expired"}:
            checks["failsBeforeModelRequest"] = details["first"]["finished"] and details["first"]["exitCode"] == 1 and details["first"]["isError"] is True and details["messageCount"] == 0
            checks["expectedRefreshAttempts"] = details["refreshCount"] == (1 if name == "expired" else 0)
        else:
            checks["heldNativeLockBeforeContender"] = details["firstRefreshReached"] and details["nativeLockPresent"] and details["refreshStartsWhileFirstBlocked"] == 1
            checks["oneRefreshAtATime"] = details["maxSimultaneousRefreshes"] == 1
            checks["oneSuccessfulRotation"] = details["successfulSyntheticRotations"] == 1
            checks["freshTokenForEveryMessage"] = details["allMessagesUsedRotatedToken"] is True
            checks["secondCompleted"] = details["second"]["finished"] and details["second"]["exitCode"] == 0 and details["second"]["fixtureReply"]
            if name == "owner_death":
                checks["ownerDeathBeforeRotationRecovered"] = details["killedOwnerReaped"] and details["refreshCount"] == 2 and details["messageCount"] == 1 and not details["lockPresentAfterReplacement"] and any(e["kind"] == "refresh_abandoned_before_rotation" for e in fixture.events)
            else:
                checks["bothCompletedOneRotation"] = details["first"]["finished"] and details["first"]["exitCode"] == 0 and details["first"]["fixtureReply"] and details["refreshCount"] == 1 and details["messageCount"] == 2
                if name == "parent_death":
                    checks["parentDeathPreservesLiveOwner"] = details["launcherKilled"] and details["nativeOwnerSurvivedParent"]
        details["checks"] = checks
        details["passed"] = all(checks.values())
        return details
    finally:
        fixture.release_refresh.set()
        for child in children:
            child.stop()
        details["allNativeChildrenReaped"] = all(child.poll() is not None for child in children)
        if "passed" in details:
            details["checks"]["allNativeChildrenReaped"] = details["allNativeChildrenReaped"]
            details["passed"] = details["passed"] and details["allNativeChildrenReaped"]
        fixture.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--claude", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--case", choices=["missing", "expired", "concurrent", "owner_death", "parent_death", "all"], default="all")
    parser.add_argument("--inside-network-namespace", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    binary = args.claude.resolve(strict=True)
    root = args.output.resolve()
    if not args.inside_network_namespace:
        # No credential/helper/provider environment is passed to the nested
        # driver. Network isolation is established before creating fake state.
        cmd = ["unshare", "--user", "--map-root-user", "--net", "--fork", sys.executable,
               str(Path(__file__).resolve()), "--inside-network-namespace", "--claude", str(binary),
               "--output", str(root), "--case", args.case]
        return subprocess.call(cmd, env={"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8"})
    subprocess.run(["ip", "link", "set", "lo", "up"], check=True)
    # Reap the native process adopted after its intermediate launcher is killed.
    if ctypes.CDLL(None, use_errno=True).prctl(36, 1, 0, 0, 0) != 0:
        raise OSError(ctypes.get_errno(), "cannot enable child subreaping")
    interfaces = sorted(name for _, name in socket.if_nameindex())
    if interfaces != ["lo"]:
        raise RuntimeError("refusing auth probe without loopback-only network namespace")
    root.mkdir(mode=0o700, parents=True, exist_ok=False)
    version = subprocess.check_output([str(binary), "--version"], env={"HOME": str(root), "PATH": "/usr/bin:/bin"}, text=True).strip()
    report = {"scope": "actual native CLI; fake login, fake OAuth rotation, synthetic provider; loopback-only namespace",
              "observedAt": datetime.now(timezone.utc).isoformat(),
              "probeSha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              "version": version, "binarySha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "interfaces": interfaces, "cases": {}}
    for name in (["missing", "expired", "concurrent", "owner_death", "parent_death"] if args.case == "all" else [args.case]):
        report["cases"][name] = run_case(name, binary, root / name)
        save(root / "receipt.json", report)
        print(json.dumps({"case": name, "result": report["cases"][name]}), flush=True)
    return 0 if all(case["passed"] for case in report["cases"].values()) else 1


if __name__ == "__main__":
    raise SystemExit(main())
