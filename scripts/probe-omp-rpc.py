#!/usr/bin/env python3
"""Probe real OMP control transport in disposable state without inherited credentials.

Idle steering can start a turn. No coding work or tools are requested; the probe
immediately aborts after observing steering acknowledgement.
"""
import argparse
import json
import os
from pathlib import Path
import queue
import subprocess
import tempfile
import threading
import time


class Rpc:
    def __init__(self, argv, env, cwd):
        self.frames = queue.Queue()
        self.sequence = 0
        self.proc = subprocess.Popen(argv, cwd=cwd, env=env, stdin=subprocess.PIPE,
                                     stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
        self.reader = threading.Thread(target=self.read, daemon=True)
        self.reader.start()

    def read(self):
        for line in self.proc.stdout:
            try:
                value = json.loads(line)
                if isinstance(value, dict):
                    self.frames.put(value)
            except json.JSONDecodeError:
                pass
        self.frames.put({"type": "process_eof"})

    def command(self, kind, **fields):
        self.sequence += 1
        ident = f"probe-{self.sequence}"
        self.proc.stdin.write(json.dumps({"type": kind, "id": ident, **fields}) + "\n")
        self.proc.stdin.flush()
        deadline = time.monotonic() + 20
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(f"No response to {kind}")
            try:
                frame = self.frames.get(timeout=remaining)
            except queue.Empty:
                raise TimeoutError(f"No response to {kind}") from None
            if frame.get("type") == "process_eof":
                raise RuntimeError(f"OMP exited before replying to {kind}")
            if frame.get("type") == "response" and frame.get("id") == ident:
                if not frame.get("success"):
                    raise RuntimeError(f"OMP rejected {kind}: {frame.get('error', 'unspecified')}")
                return frame.get("data")

    def close(self):
        if not self.proc.stdin.closed:
            self.proc.stdin.close()
        try:
            code = self.proc.wait(timeout=10)
            return {"exit_code": code, "forced": False}
        except subprocess.TimeoutExpired:
            self.proc.kill()
            return {"exit_code": self.proc.wait(timeout=5), "forced": True}
        finally:
            self.reader.join(timeout=1)
            self.proc.stdout.close()


def state_summary(state):
    summary = {key: state.get(key) for key in (
        "sessionId", "sessionName", "isStreaming", "isCompacting", "queuedMessageCount", "messageCount"
    )}
    model = state.get("model") or {}
    summary["model"] = {key: model.get(key) for key in ("provider", "id")}
    return summary


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    binary = str(args.binary.resolve(strict=True))
    version = subprocess.run([binary, "--version"], capture_output=True, text=True, check=True, timeout=10)
    report = {"version": version.stdout.strip(), "scope": "Isolated control probe; idle steering may start a turn; no inherited provider credentials or enabled tools", "observations": {}}
    observations = report["observations"]
    with tempfile.TemporaryDirectory(prefix="crewroom-omp-rpc-") as tmp:
        root = Path(tmp)
        private = root / "private-agent"
        work = root / "work"
        private.mkdir()
        work.mkdir()
        # Do not inherit provider credentials or user OMP configuration.
        env = {key: os.environ[key] for key in ("PATH", "HOME", "USER", "TMPDIR", "LANG", "SHELL") if key in os.environ}
        env["PI_CODING_AGENT_DIR"] = str(private)
        env["PI_CODING_AGENT_SESSION_DIR"] = str(root / "sessions")
        argv = [binary, "--mode", "rpc", "--no-ui", "--no-tools", "--no-extensions", "--no-rules",
                "--no-skills", "--no-lsp", "--no-title", "--no-pty", "--cwd", str(work),
                "--session-dir", str(root / "sessions"), "--max-time", "30s"]
        rpc = None
        try:
            rpc = Rpc(argv, env, work)
            before = rpc.command("get_state")
            observations["initial"] = state_summary(before)
            assert before.get("sessionId") and before.get("isStreaming") is False
            rpc.command("set_session_name", name="Crewroom control contract probe")
            rpc.command("steer", message="Transport probe only. Do no work. Reply with the word acknowledged if a turn starts.")
            queued = rpc.command("get_state")
            observations["after_steer_ack"] = state_summary(queued)
            observations["idle_steer_started_turn"] = queued.get("isStreaming") is True
            rpc.command("abort")
            stopped = rpc.command("get_state")
            observations["after_abort_ack"] = state_summary(stopped)
            assert stopped.get("isStreaming") is False
            messages = rpc.command("get_messages")
            if isinstance(messages, dict):
                messages = messages.get("messages", [])
            observations["assistant_stop_reasons"] = [message.get("stopReason") for message in messages
                                                       if message.get("role") == "assistant"]
            session = Path(stopped["sessionFile"])
            assert session.is_relative_to(root), "Session unexpectedly stored outside isolated directory"
            assert session.is_file(), "Session was not persisted"
            observations["first_shutdown"] = rpc.close()
            rpc = None
            assert observations["first_shutdown"] == {"exit_code": 0, "forced": False}
            rpc = Rpc([*argv, "--resume", str(session)], env, work)
            resumed = rpc.command("get_state")
            observations["reopened"] = state_summary(resumed)
            assert resumed["sessionId"] == before["sessionId"], "Session identity changed during reopen"
            assert resumed.get("sessionName") == "Crewroom control contract probe"
            observations["same_session_reopened"] = True
            report["passed"] = True
        except Exception as exc:
            report.update(passed=False, error=f"{type(exc).__name__}: {exc}")
        finally:
            if rpc is not None:
                observations["final_shutdown"] = rpc.close()
                if observations["final_shutdown"] != {"exit_code": 0, "forced": False}:
                    report["passed"] = False
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
