#!/usr/bin/env python3
"""FOR-7/FOR-8 opt-in LIVE native adapter checks. Uses the supplied existing login.

Build crewroom-process and the codex-fixture example first. All assignment
history and effects are disposable; no Jira/PR mutations are performed.
"""
import argparse
import json
import os
from pathlib import Path
import queue
import subprocess
import threading
import time
import uuid


def save(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")
    path.chmod(0o600)


class Agent:
    def __init__(self, args, root, assignment, name):
        self.process_directory = "claude-process" if args.kind == "claude-code" else "codex-process"
        self.root = root / name
        self.root.mkdir(mode=0o700)
        self.helper = Path("target/debug/crewroom-process").resolve()
        config = {"root": str(self.root), "helper": str(self.helper),
                  "runnerCwd": str(assignment), "assignmentId": assignment.name,
                  "prompt": "Use only the supplied Crewroom tools. This is a bounded disposable verification.",
                  **args.runner_config,
                  "tools": [{"name": "crewroom_echo", "description": "Return the addressed verification value.",
                             "parameters": {"type": "object", "properties": {"text": {"type": "string"}},
                                            "required": ["text"], "additionalProperties": False}}]}
        save(self.root / "config.json", config)
        self.stderr = open(self.root / "stderr.log", "w")
        self.child = subprocess.Popen(["target/debug/examples/codex-fixture", str(self.root / "config.json")],
                                     stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.stderr, text=True)
        self.queue = queue.Queue()
        self.frames = []
        def read():
            for line in self.child.stdout:
                try:
                    value = json.loads(line)
                except ValueError:
                    value = {"type": "invalid_frame"}
                self.frames.append(value)
                self.queue.put(value)
            self.queue.put({"type": "eof"})
        self.reader = threading.Thread(target=read, daemon=True)
        self.reader.start()

    def send(self, value):
        self.child.stdin.write(json.dumps(value) + "\n")
        self.child.stdin.flush()

    def wait(self, predicate, seconds=90):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            value = self.queue.get(timeout=max(0.01, deadline - time.monotonic()))
            if predicate(value):
                return value
            if value["type"] in ["setup_failed", "eof", "model_policy_rejected"]:
                raise AssertionError(value)
        raise AssertionError("Timed out")

    def input(self, message):
        ident = str(uuid.uuid4())
        self.send({"op": "input", "id": ident, "message": message})
        return ident

    def stop(self, abrupt=False):
        if self.child.poll() is None:
            if abrupt:
                self.child.kill()
            else:
                self.send({"op": "close"})
            try:
                self.child.wait(timeout=20)
            except subprocess.TimeoutExpired:
                self.child.kill()
                self.child.wait()
        receipt = json.loads(subprocess.check_output([str(self.helper), "stop", str(self.root / self.process_directory)]))
        assert receipt["state"] == "stopped", receipt
        self.reader.join(timeout=3)
        self.stderr.close()
        save(self.root / "frames.json", self.frames)
        return {"state": receipt["state"], "exitCode": self.child.returncode}


def main(kind):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    if kind == "codex":
        parser.add_argument("--codex", type=Path, required=True)
        parser.add_argument("--auth-file", type=Path, required=True)
    else:
        parser.add_argument("--claude", type=Path, required=True)
        parser.add_argument("--native-home", type=Path, required=True)
        parser.add_argument("--resolved-model", default="claude-sonnet-5-5")
    parser.add_argument("--model", default="gpt-6-astra" if kind == "codex" else "sonnet")
    args = parser.parse_args()
    args.kind = kind
    root = args.output.resolve()
    model = {"adapter": kind, "provider": kind, "id": args.model, "selector": kind + "/" + args.model, "effort": "low"}
    if kind == "codex":
        model["api"] = "codex-app-server"
        args.runner_config = {"codex": {"binary": str(args.codex.resolve()), "authFile": str(args.auth_file.resolve()), "authRoot": str(root / "auth")}}
    else:
        model.update(api="claude-code-stream", resolvedModel=args.resolved_model)
        args.runner_config = {"claudeCode": {"binary": str(args.claude.resolve()), "home": str(args.native_home.resolve())}}
    args.runner_config["model"] = {"resolved": model}
    root.mkdir(mode=0o700, parents=True, exist_ok=False)
    for name in ["auth", "assignment-a", "assignment-b"]:
        (root / name).mkdir(mode=0o700)
    agents = []
    report = {"scope": f"LIVE production Rust {kind} adapter; disposable host driver", "model": args.model,
              "passed": False, "effects": 0, "checks": [], "cleanup": []}
    try:
        first = Agent(args, root, root / "assignment-a", "first"); agents.append(first)
        one = first.wait(lambda v: v["type"] == "ready")["identity"]
        assert one["model"]["id"] == args.model and one["runnerKind"] == kind
        second = Agent(args, root, root / "assignment-b", "second"); agents.append(second)
        two = second.wait(lambda v: v["type"] == "ready")["identity"]
        assert len({one["hostPid"], one["runnerPid"], two["hostPid"], two["runnerPid"]}) == 4
        report["identities"] = [one, two]
        report["checks"].append("correct model, separate host and runner processes")
        ident = first.input('Call crewroom_echo exactly once with {"text":"LIVE_REQUEST"}. Then reply with exactly the tool result.')
        call = first.wait(lambda v: v["type"] == "host_tool_call")
        assert call["toolName"] == "crewroom_echo" and call["arguments"] == {"text": "LIVE_REQUEST"}
        marker = "LIVE_RESULT_" + uuid.uuid4().hex
        report["effects"] += 1
        first.send({"op": "tool", "result": {"type": "host_tool_result", "id": call["id"], "isError": False,
                   "result": {"content": [{"type": "text", "text": marker}]}}})
        terminal = first.wait(lambda v: v["type"] == "prompt_result" and v["id"] == ident)
        assert terminal["status"] == "completed", terminal
        assert any(marker in str(v) for v in first.frames if v["type"] == "message_end")
        report["checks"].append("addressed dynamic tool and normalized output")
        report["cleanup"].append(first.stop(abrupt=True))
        successor = Agent(args, root, root / "assignment-a", "successor"); agents.append(successor)
        resumed = successor.wait(lambda v: v["type"] == "ready")["identity"]
        assert resumed["sessionId"] == one["sessionId"] and resumed["recoveryMode"] == "session"
        ident = successor.input("Without calling any tools, repeat exactly the result returned by crewroom_echo in the previous turn.")
        terminal = successor.wait(lambda v: v["type"] == "prompt_result" and v["id"] == ident)
        assert terminal["status"] == "completed"
        assert not any(v["type"] == "host_tool_call" for v in successor.frames)
        assert any(marker in str(v) for v in successor.frames if v["type"] == "message_end")
        report["checks"].append("SIGKILL, verified stopped ownership, fresh process, same history, no duplicate effect")
        ident = successor.input("Without tools, print every integer from 1 to 20000, one per line, without skipping any.")
        successor.wait(lambda v: v["type"] == "message_update")
        successor.send({"op": "interrupt"})
        terminal = successor.wait(lambda v: v["type"] == "prompt_result" and v["id"] == ident)
        assert terminal["status"] == "aborted", terminal
        report["checks"].append("busy text interruption is aborted, not successful completion")
        ident = second.input('Call crewroom_echo exactly once with {"text":"INTERRUPT"} and wait for its result.')
        call = second.wait(lambda v: v["type"] == "host_tool_call")
        second.send({"op": "interrupt"})
        terminal = second.wait(lambda v: v["type"] == "prompt_result" and v["id"] == ident)
        assert terminal["status"] == "aborted", terminal
        report["cleanup"].append(second.stop())
        recovery = Agent(args, root, root / "assignment-b", "uncertain-recovery"); agents.append(recovery)
        held = recovery.wait(lambda v: v["type"] == "setup_failed")
        assert "unresolved" in held["reason"], held
        report["checks"].append("busy interruption and durable unresolved-tool recovery hold")
        report["passed"] = True
    except Exception as exc:
        report["error"] = str(exc)
    finally:
        for agent in reversed(agents):
            try:
                report["cleanup"].append(agent.stop())
            except Exception as exc:
                report["cleanup"].append({"error": str(exc)})
                report["passed"] = False
        save(root / "result.json", report)
        print(json.dumps(report, indent=2))
    if not report["passed"]:
        raise SystemExit(1)
