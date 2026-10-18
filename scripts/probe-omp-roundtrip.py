#!/usr/bin/env python3
"""Run a bounded native OMP coordinator -> two specialists -> coordinator probe.

Uses an existing models.yml privately; never prints its contents. Raw RPC events
and sessions stay in the explicitly selected local output directory.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import queue
import shutil
import subprocess
import threading
import time
import uuid


spec = importlib.util.spec_from_file_location("rpc_probe", Path(__file__).with_name("probe-omp-rpc.py"))
rpc_probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(rpc_probe)


class RecordedRpc(rpc_probe.Rpc):
    def __init__(self, argv, env, cwd, output):
        self.frames = queue.Queue()
        self.sequence = 0
        self.events = []
        self.log = (output / "events.jsonl").open("w")
        self.errors = (output / "stderr.log").open("w")
        self.proc = subprocess.Popen(argv, cwd=cwd, env=env, stdin=subprocess.PIPE,
                                     stdout=subprocess.PIPE, stderr=self.errors, text=True)
        self.reader = threading.Thread(target=self.read, daemon=True)
        self.reader.start()

    def read(self):
        for line in self.proc.stdout:
            self.log.write(line)
            self.log.flush()
            try:
                frame = json.loads(line)
                if isinstance(frame, dict):
                    self.events.append(frame)
                    self.frames.put(frame)
            except json.JSONDecodeError:
                pass
        self.frames.put({"type": "process_eof"})

    def close(self):
        result = super().close()
        self.log.close()
        self.errors.close()
        return result


def unpack(frame):
    if frame.get("type") == "subagent_event":
        payload = frame.get("payload", {})
        return payload.get("id", "unknown"), payload.get("event", {})
    return "coordinator", frame


def text_content(message):
    return "\n".join(part.get("text", "") for part in message.get("content", []) if part.get("type") == "text")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--models-file", type=Path, default=Path.home() / ".omp/agent/models.yml")
    parser.add_argument("--binary", default=str(Path.home() / '.bun/bin/omp'))
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    root.chmod(0o700)
    private, work = root / "private-agent", root / "work"
    private.mkdir(mode=0o700)
    work.mkdir()
    role_dir = work / ".omp/agents"
    role_dir.mkdir(parents=True)
    shutil.copyfile(args.models_file, private / "models.yml")
    (private / "models.yml").chmod(0o600)
    (private / "mcp.json").write_text('{"mcpServers": {}}\n')
    (private / "config.yml").write_text("async:\n  enabled: true\ntask:\n  batch: true\n")
    role = f"""---
name: message-probe
description: A specialist for the Crewroom native message round-trip experiment
tools: [read, write]
spawns: []
model: {args.model}
thinkingLevel: low
---
You participate only in the requested agent messaging experiment.
Use write with an agent:// recipient URI to actually send messages. Only agent://
reads and writes are permitted. Do not read or write files or URLs, invoke external
services, create agents, or perform coding work. Use the assigned protocol exactly.
On initial setup reply READY and finish your turn. Later incoming agent messages
must trigger the specified response through write, not just descriptive prose.
Do not acknowledge acknowledgements or create a chat loop. After sending a required
message, finish your turn immediately. Keep answers brief.
"""
    (role_dir / "message-probe.md").write_text(role)
    trace = "crewroom-" + uuid.uuid4().hex[:16]
    prompt = f"""Run this real native-agent messaging test. Trace: {trace}.
Use exactly two task agents, both agent="message-probe", named SpecialistOne and
SpecialistTwo. Their separate task instructions must establish this protocol:
- SpecialistOne: initially READY and finish. On START from the coordinator, remember
  that sender and the supplied SpecialistTwo address. Write agent://<SpecialistTwo>
  with FORWARD <trace>. On a reply S2_ACK <trace> from SpecialistTwo, write back to
  the original coordinator sender with S1_RELAY S2_ACK <trace>. Finish after each send.
- SpecialistTwo: initially READY and finish. On FORWARD <trace> from SpecialistOne,
  write agent://<actual sender> with S2_ACK <trace>, then finish. Never send directly
  to the coordinator except the ordinary initial READY task completion.
Use a task context covering only this message experiment. Do not supply the actual
trace to SpecialistTwo's initial task; it must learn the trace from SpecialistOne.
Once both agents exist, use their actual returned IDs, not guesses. You must write
agent://<SpecialistOne> with START {trace}, the actual SpecialistTwo ID, and an
instruction to return the reply to the actual sender recorded on the incoming
message. Finish your turn while waiting for the reply; do not poll agent outputs.
Only after you actually RECEIVE S1_RELAY S2_ACK {trace} from SpecialistOne, output
ROUNDTRIP_COMPLETE {trace}. Do not fabricate receipts, relay on the host's behalf,
skip a hop, broadcast, or send START twice. If something fails, report the failure.
Only use task, read agent://, and write agent://. No files, external services,
research, coding, tests, or additional agents.
"""
    (root / "prompt.txt").write_text(prompt)
    env = dict(os.environ)
    env.update(PI_CODING_AGENT_DIR=str(private), PI_CODING_AGENT_SESSION_DIR=str(root / "sessions"))
    argv = [args.binary, "--mode", "rpc", "--no-ui", "--tools", "task,read,write",
            "--no-extensions", "--no-rules", "--no-skills", "--no-lsp", "--no-title", "--no-pty",
            "--cwd", str(work), "--session-dir", str(root / "sessions"),
            "--model", args.model, "--thinking", "low", "--max-time", "3m",
            "--system-prompt", "You are the coordinator for a bounded native agent-message experiment. Follow the user's protocol exactly. Real sends and observed replies are required."]
    report = {"trace": trace, "requested_model": args.model, "scope": "Live native session message round trip only", "passed": False}
    rpc = None
    try:
        rpc = RecordedRpc(argv, env, work, root)
        state = rpc.command("get_state")
        report["coordinator_session_id"] = state["sessionId"]
        report["root_tools"] = [tool["name"] for tool in state.get("dumpTools", [])]
        if set(report["root_tools"]) != {"task", "read", "write"}:
            raise RuntimeError("Unexpected tool surface; refusing to start")
        task_tool = next(tool for tool in state["dumpTools"] if tool["name"] == "task")
        if "`message-probe`" not in task_tool.get("description", ""):
            raise RuntimeError("Temporary specialist definition was not discovered; refusing to start")
        rpc.command("set_subagent_subscription", level="events")
        rpc.command("prompt", message=prompt)
        started = time.monotonic()
        writes = []
        final = None
        while time.monotonic() - started < 180:
            try:
                frame = rpc.frames.get(timeout=5)
            except queue.Empty:
                if rpc.proc.poll() is not None:
                    raise RuntimeError("OMP exited before round-trip completion")
                continue
            who, event = unpack(frame)
            if event.get("type") == "tool_execution_end" and event.get("toolName") == "task":
                result_text = text_content(event.get("result", {}))
                if "failed preflight" in result_text:
                    raise RuntimeError("Specialist creation failed preflight; see private event log")
            if event.get("type") == "tool_execution_start":
                name, params = event.get("toolName"), event.get("args", {})
                if name == "write":
                    item = {"sender": who, "tool_call_id": event.get("toolCallId"), **params}
                    writes.append(item)
                    print(json.dumps({"send": item}), flush=True)
                elif name == "task":
                    print(json.dumps({"spawn": [t.get("name") for t in params.get("tasks", [])]}), flush=True)
                if name in ("read", "write") and not str(params.get("path", "")).startswith("agent://"):
                    raise RuntimeError("Non-agent tool path requested; stopping experiment")
            if event.get("type") == "message_end":
                message = event.get("message", {})
                if message.get("role") == "assistant":
                    content = text_content(message)
                    if content:
                        print(json.dumps({"agent": who, "text": content[:500]}), flush=True)
                    if who == "coordinator" and f"ROUNDTRIP_COMPLETE {trace}" in content:
                        final = content
                        break
                    if who == "coordinator" and message.get("stopReason") == "error":
                        raise RuntimeError("Coordinator provider error; see private event log")
        report["elapsed_seconds"] = round(time.monotonic() - started, 2)
        report["writes"] = writes
        report["final"] = final
        report["subagents"] = rpc.command("get_subagents").get("subagents", [])
        (root / "root-messages.json").write_text(json.dumps(rpc.command("get_messages"), indent=2))
        for index, agent in enumerate(report["subagents"]):
            ident = agent.get("id")
            if ident:
                history = rpc.command("get_subagent_messages", subagentId=ident)
                (root / f"specialist-{index + 1}-messages.json").write_text(json.dumps(history, indent=2))
        # A model's completion claim is only a candidate result. Verify actual
        # send results and recipient transcripts from the retained evidence.
        report["completion_claim_observed"] = final is not None
        if final is None:
            report["error"] = "No completed round-trip claim within the bounded experiment"
        rpc.command("abort")
    except Exception as exc:
        report["error"] = f"{type(exc).__name__}: {exc}"
        if rpc is not None:
            try:
                rpc.command("abort")
            except Exception:
                pass
    finally:
        if rpc is not None:
            report["shutdown"] = rpc.close()
        (private / "models.yml").unlink(missing_ok=True)
        (root / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"output": str(root), "claim": report.get("completion_claim_observed"), "error": report.get("error"), "shutdown": report.get("shutdown")}), flush=True)
    return 0 if report.get("completion_claim_observed") else 1


if __name__ == "__main__":
    raise SystemExit(main())
