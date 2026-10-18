#!/usr/bin/env python3
"""Bounded, externally observable message experiment with three independent OMP processes.

Relaycast is a separately started loopback service. This adapter exposes one host
tool to each OMP process and delivers persisted mailbox messages through RPC.
It never invents a protocol response on an agent's behalf.
"""
import argparse
import datetime as dt
import hashlib
import http.server
import json
import os
from pathlib import Path
import queue
import shutil
import subprocess
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid

NAMES = ("Coordinator", "SpecialistOne", "SpecialistTwo")
TOOL = {"name": "send_message", "label": "Send external message", "loadMode": "essential",
        "description": "Send an addressed message through the external Relaycast mailbox. Returns durable storage receipt, not proof the recipient acted.",
        "parameters": {"type": "object", "properties": {
            "to": {"type": "string", "enum": list(NAMES)}, "body": {"type": "string"}},
            "required": ["to", "body"], "additionalProperties": False}}


def now():
    return dt.datetime.now(dt.timezone.utc).isoformat()


def message_text(message):
    content = message.get("content", [])
    if isinstance(content, str):
        return content
    return "\n".join(p.get("text", "") for p in content if p.get("type") == "text")


class Relay:
    def __init__(self, url, private):
        parsed = urllib.parse.urlparse(url)
        if parsed.scheme != "http" or parsed.hostname != "127.0.0.1":
            raise ValueError("This experiment requires a loopback-only Relaycast endpoint")
        self.url, self.private, self.tokens, self.identities = url.rstrip("/"), private, {}, {}
        self.observer_token, self.observer_cursor, self.observer_events = None, 0, []

    def request(self, method, path, body=None, token=None, key=None):
        headers = {"Content-Type": "application/json"}
        if token:
            headers["Authorization"] = "Bearer " + token
        if key:
            headers["Idempotency-Key"] = key
        request = urllib.request.Request(self.url + path, method=method, headers=headers,
                                         data=None if body is None else json.dumps(body).encode())
        try:
            with urllib.request.urlopen(request, timeout=10) as response:
                result = json.load(response)
        except urllib.error.HTTPError as exc:
            raise RuntimeError(f"Relaycast {method} {path} returned HTTP {exc.code}: {exc.read().decode()[:400]}") from None
        if result.get("ok") is False:
            raise RuntimeError(f"Relaycast rejected {method} {path}")
        return result.get("data", result)

    def setup(self):
        workspace = self.request("POST", "/v1/workspaces", {"name": "crewroom-probe-" + uuid.uuid4().hex[:12]})
        workspace_key = workspace.get("api_key") or workspace.get("key")
        if not workspace_key:
            raise RuntimeError("Workspace response has no API key")
        for name in NAMES:
            agent = self.request("POST", "/v1/agents", {"name": name}, token=workspace_key)
            self.tokens[name] = agent["token"]
            self.identities[name] = agent["id"]
        observer = self.request("POST", "/v1/observer-tokens", {
            "name": "Crewroom passive monitor", "scopes": ["stream:read", "dms:read"],
            "filters": {"include_dms": True}}, token=workspace_key)
        self.observer_token = observer["token"]
        secret_file = self.private / "relaycast-credentials.json"
        secret_file.write_text(json.dumps({"workspace": workspace, "tokens": self.tokens, "observer_token": self.observer_token}))
        secret_file.chmod(0o600)

    def send(self, sender, recipient, body, key):
        return self.request("POST", "/v1/dm", {"to": recipient, "text": body, "mode": "wait"},
                            token=self.tokens[sender], key=key)

    def deliveries(self, recipient, status=None):
        result = self.request("GET", "/v1/deliveries" + ("?status=" + status if status else ""), token=self.tokens[recipient])
        return result if isinstance(result, list) else result["deliveries"]

    def ack(self, recipient, ident):
        return self.request("POST", f"/v1/deliveries/{ident}/ack", {}, token=self.tokens[recipient])

    def history(self, participant, conversation):
        return self.request("GET", f"/v1/dm/{conversation}/messages", token=self.tokens[participant])

    def observe(self):
        result = self.request("GET", f"/v1/workspace/events?since={self.observer_cursor}&limit=100", token=self.observer_token)
        self.observer_cursor = result["next_since"]
        self.observer_events.extend(result["events"])
        return result["events"]


class Omp:
    def __init__(self, name, binary, model, models_file, root, events):
        self.name, self.events, self.frames, self.responses = name, events, [], {}
        self.seq, self.busy, self.closed, self.lock = 0, False, False, threading.Lock()
        self.root = root / name
        private, work = self.root / "private", self.root / "work"
        private.mkdir(parents=True, mode=0o700)
        work.mkdir()
        shutil.copyfile(models_file, private / "models.yml")
        (private / "models.yml").chmod(0o600)
        (private / "mcp.json").write_text('{"mcpServers": {}}\n')
        (private / "config.yml").write_text("async:\n  enabled: false\n")
        self.log = (self.root / "rpc-events.jsonl").open("w")
        self.inputs = (self.root / "rpc-inputs.jsonl").open("w")
        self.stderr = (self.root / "stderr.log").open("w")
        env = dict(os.environ)
        env.update(PI_CODING_AGENT_DIR=str(private), PI_CODING_AGENT_SESSION_DIR=str(self.root / "sessions"))
        argv = [binary, "--mode", "rpc", "--no-ui", "--no-tools", "--no-extensions", "--no-rules",
                "--no-skills", "--no-lsp", "--no-title", "--no-pty", "--cwd", str(work),
                "--session-dir", str(self.root / "sessions"), "--model", model, "--thinking", "low",
                "--max-time", "3m", "--system-prompt",
                f"You are {name} in a bounded external-message experiment. Follow only the assigned protocol. Use send_message for actual sends. Never invent a receipt. After a successful send_message call, finish with the exact text SENT. Always provide a nonempty final answer. Do not send extra message-tool acknowledgements. No coding, files, research, or other work."]
        self.proc = subprocess.Popen(argv, env=env, cwd=work, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                     stderr=self.stderr, text=True, start_new_session=True)
        self.thread = threading.Thread(target=self.read, daemon=True)
        self.thread.start()

    def read(self):
        for line in self.proc.stdout:
            self.log.write(line)
            self.log.flush()
            try:
                frame = json.loads(line)
            except json.JSONDecodeError:
                continue
            self.frames.append(frame)
            if frame.get("type") == "response" and frame.get("id") in self.responses:
                self.responses[frame["id"]].put(frame)
            else:
                self.events.put((self.name, frame))
        self.events.put((self.name, {"type": "process_eof"}))

    def send(self, frame):
        with self.lock:
            self.inputs.write(json.dumps(frame) + "\n")
            self.inputs.flush()
            self.proc.stdin.write(json.dumps(frame) + "\n")
            self.proc.stdin.flush()

    def command(self, kind, **fields):
        self.seq += 1
        ident = f"{self.name}-{self.seq}"
        reply = self.responses[ident] = queue.Queue()
        self.send({"type": kind, "id": ident, **fields})
        try:
            result = reply.get(timeout=20)
            if not result.get("success"):
                raise RuntimeError(f"{self.name}: {kind} rejected: {result.get('error')}")
            return result.get("data")
        finally:
            self.responses.pop(ident, None)

    def prompt(self, message):
        self.busy = True
        self.command("prompt", message=message)

    def close(self):
        if self.closed:
            return
        self.closed = True
        try:
            if self.proc.poll() is None:
                self.command("abort")
                self.proc.stdin.close()
                try:
                    self.proc.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    self.proc.kill()
                    self.proc.wait(timeout=5)
        finally:
            self.thread.join(timeout=1)
            self.log.close()
            self.inputs.close()
            self.stderr.close()
            (self.root / "private/models.yml").unlink(missing_ok=True)
        return self.proc.returncode


HTML = """<!doctype html><html><meta charset="utf-8"><title>OMP message experiment</title>
<style>body{font:16px system-ui;margin:32px;background:#f4f7f8;color:#203039}h1{font-size:25px}#agents{display:flex;gap:16px}article{flex:1;background:white;border:1px solid #cbd8df;border-radius:10px;padding:20px;min-width:0}pre{white-space:pre-wrap;word-break:break-word;font-size:13px}table{width:100%;border-collapse:collapse;background:white}td,th{text-align:left;padding:12px;border-bottom:1px solid #ddd}small{color:#546872}#status{margin:20px 0}</style>
<h1>Three independent OMP processes</h1><small>External Relaycast messages · direct RPC progress · passive observation</small>
<div id="status"></div><div id="agents"></div><h2>Message history</h2><table><thead><tr><th>From → To</th><th>Message</th><th>Receipt</th></tr></thead><tbody id="messages"></tbody></table>
<script>const initial=__SNAPSHOT__;const esc=s=>String(s??'').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
function render(s){document.querySelector('#status').textContent=s.status;document.querySelector('#agents').innerHTML=Object.entries(s.agents).map(([n,a])=>`<article><h2>${esc(n)}</h2><p>PID ${esc(a.pid)} · ${esc(a.state)}</p><small>Session ${esc(a.session_id)}</small><pre>${esc(a.activity.join('\\n'))}</pre></article>`).join('');document.querySelector('#messages').innerHTML=s.messages.map(m=>`<tr><td>${esc(m.from)} → ${esc(m.to)}</td><td>${esc(m.body)}</td><td>${esc(m.receipt)}</td></tr>`).join('')}
render(initial);if(location.protocol==='http:')setInterval(()=>fetch('/snapshot').then(r=>r.json()).then(render).catch(()=>{}),500);</script></html>"""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--relay-url", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--model", required=True)
    parser.add_argument("--models-file", type=Path, default=Path.home() / ".omp/agent/models.yml")
    parser.add_argument("--binary", default=str(Path.home() / '.bun/bin/omp'))
    parser.add_argument("--rounds", type=int, default=3)
    parser.add_argument("--start-delay", type=float, default=10)
    parser.add_argument("--view-seconds", type=float, default=30)
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False, mode=0o700)
    events, agents = queue.Queue(), {}
    report = {"started_at": now(), "passed": False, "candidate": "Relaycast", "rounds": [], "sends": [],
              "received": [], "acks": [], "observer_checks": [], "processes": {}, "shutdown": {}}
    snapshot = {"status": "Starting independent processes", "agents": {}, "messages": []}
    class Monitor(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path not in ("/", "/snapshot"):
                self.send_error(404)
                return
            data = json.dumps(snapshot).replace("</", "<\\/")
            body = (data if self.path == "/snapshot" else HTML.replace("__SNAPSHOT__", data)).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json" if self.path == "/snapshot" else "text/html; charset=utf-8")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        def log_message(self, *unused):
            pass
    monitor = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Monitor)
    threading.Thread(target=monitor.serve_forever, daemon=True).start()
    monitor_url = f"http://127.0.0.1:{monitor.server_port}"
    (root / "monitor-url.txt").write_text(monitor_url)
    print(json.dumps({"monitor": monitor_url, "output": str(root)}), flush=True)
    backend = Relay(args.relay_url, root)
    pending, seen, finals = {}, set(), set()
    next_poll = {name: 0.0 for name in NAMES}
    audit = (root / "observations.jsonl").open("w")

    def record(kind, **data):
        entry = {"time": now(), "kind": kind, **data}
        audit.write(json.dumps(entry) + "\n")
        audit.flush()
        if kind in {"send", "received", "round_complete", "failure"}:
            print(json.dumps(entry), flush=True)

    def handle(name, frame):
        agent = agents[name]
        kind = frame.get("type")
        panel = snapshot["agents"][name]
        if kind == "process_eof" and not agent.closed:
            raise RuntimeError(f"{name} unexpectedly exited")
        if kind in {"subagent_event", "subagent_lifecycle", "irc_message"}:
            raise RuntimeError("Unexpected native-team activity")
        if kind == "auto_retry_end" and frame.get("success") is False:
            raise RuntimeError(f"{name} exhausted automatic provider retries; see private RPC log")
        if kind == "agent_start":
            panel["state"] = "running"
        if kind == "prompt_result":
            if frame.get("status") != "completed":
                raise RuntimeError(f"{name} prompt failed: {frame.get('status')}")
            agent.busy = False
            panel["state"] = "waiting"
        if kind == "host_tool_call":
            if frame.get("toolName") != "send_message":
                raise RuntimeError("Unexpected host tool")
            params = frame["arguments"]
            recipient, body = params["to"], params["body"]
            if recipient not in NAMES or recipient == name or len(body) > 2000:
                raise RuntimeError("Invalid message destination or size")
            stored = backend.send(name, recipient, body, hashlib.sha256((name + frame["toolCallId"]).encode()).hexdigest())
            send = {"from": name, "to": recipient, "body": body, "message_id": stored["id"],
                    "conversation_id": stored["conversation_id"], "tool_call_id": frame["toolCallId"]}
            report["sends"].append(send)
            # A stored send is a wake-up hint; the recipient still reads the
            # durable mailbox. Idle reconciliation is bounded to one poll/2s.
            next_poll[recipient] = 0.0
            snapshot["messages"].append({**send, "receipt": "stored"})
            record("send", **send)
            agent.send({"type": "host_tool_result", "id": frame["id"], "result": {
                "content": [{"type": "text", "text": json.dumps({"stored": True, "message_id": stored["id"], "to": recipient})}],
                "details": {"message_id": stored["id"]}}})
        if kind == "tool_execution_start":
            if frame.get("toolName") != "send_message":
                raise RuntimeError("Unexpected native tool execution")
            panel["activity"].append("Tool: send_message " + json.dumps(frame.get("args", {})))
        if kind == "message_end":
            message = frame.get("message", {})
            text = message_text(message)
            if message.get("role") == "assistant":
                if message.get("stopReason") == "error":
                    raise RuntimeError(f"{name} provider failed")
                if text:
                    panel["activity"].append(text)
                    if name == "Coordinator" and text.startswith("ROUNDTRIP_COMPLETE "):
                        finals.add(text.strip().split()[-1])
            if message.get("role") == "user":
                try:
                    envelope = json.loads(text)
                except (ValueError, TypeError):
                    envelope = {}
                ident = envelope.get("message_id")
                if ident in pending:
                    target, item, expected = pending.pop(ident)
                    if target != name or envelope != expected:
                        raise RuntimeError("Recipient transcript differs from mailbox payload")
                    report["received"].append({"recipient": name, "message_id": ident, "envelope": envelope})
                    receipt = backend.ack(name, item["id"])
                    if receipt["status"] != "acked":
                        raise RuntimeError("Mailbox acknowledgement failed")
                    report["acks"].append({"recipient": name, "message_id": ident, "delivery_id": item["id"], "status": receipt["status"]})
                    for row in snapshot["messages"]:
                        if row["message_id"] == ident:
                            row["receipt"] = "observed in recipient session; mailbox acked"
                    panel["activity"].append("Received from " + envelope["from"] + ": " + envelope["body"])
                    record("received", recipient=name, message_id=ident, body=envelope["body"])
            panel["activity"] = panel["activity"][-20:]

    def tick():
        while True:
            try:
                who, frame = events.get_nowait()
            except queue.Empty:
                break
            handle(who, frame)
        for name, agent in agents.items():
            if agent.busy or time.monotonic() < next_poll[name]:
                continue
            next_poll[name] = time.monotonic() + 2.0
            inbox = backend.deliveries(name)
            for item in inbox:
                message = item.get("message") or {}
                ident = message.get("id")
                if ident in seen:
                    continue
                # Verify repeated passive reads do not consume or acknowledge it.
                backend.observe()
                again = backend.deliveries(name)
                unchanged = any(d["id"] == item["id"] and d["status"] == item["status"] for d in again)
                report["observer_checks"].append({"delivery_id": item["id"], "unchanged_after_read": unchanged})
                if not unchanged:
                    raise RuntimeError("Observer consumed a pending delivery")
                envelope = {"transport": "external-relaycast", "message_id": ident,
                            "from": message["agent_name"], "to": name, "body": message["text"]}
                pending[ident] = (name, item, envelope)
                seen.add(ident)
                agent.prompt(json.dumps(envelope))
                break
        time.sleep(0.05)

    def until(predicate, seconds=90):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            tick()
            if predicate():
                return
        raise TimeoutError("Experiment deadline exceeded")

    try:
        backend.setup()
        report["binary_version"] = subprocess.check_output([args.binary, "--version"], text=True).strip()
        for name in NAMES:
            agents[name] = Omp(name, args.binary, args.model, args.models_file, root, events)
            agent = agents[name]
            state = agent.command("get_state")
            if state.get("dumpTools"):
                raise RuntimeError("Unrequested tools available before registration")
            agent.command("set_host_tools", tools=[TOOL])
            state = agent.command("get_state")
            if [t["name"] for t in state.get("dumpTools", [])] != ["send_message"]:
                raise RuntimeError("External message tool was not the only available tool")
            info = {"pid": agent.proc.pid, "process_group": os.getpgid(agent.proc.pid), "session_id": state["sessionId"],
                    "model": {k: state["model"].get(k) for k in ("provider", "id")}}
            report["processes"][name] = info
            snapshot["agents"][name] = {**info, "state": "ready", "activity": []}
        if len({a.proc.pid for a in agents.values()}) != 3 or len({i["session_id"] for i in report["processes"].values()}) != 3:
            raise RuntimeError("Processes or sessions are not distinct")
        report["ps_snapshot"] = subprocess.check_output(["ps", "-p", ",".join(str(a.proc.pid) for a in agents.values()), "-o", "pid=,ppid=,pgid=,comm="], text=True)
        print(json.dumps({"processes": report["processes"]}), flush=True)
        snapshot["status"] = "Three independent processes ready; preparing protocol"
        instructions = {
            "Coordinator": "You coordinate this message-only experiment. Initially reply READY. On a user RUN <trace>, call send_message to SpecialistOne with body START <trace>, then reply SENT and finish. When an external envelope from SpecialistOne has body S1_RELAY S2_ACK <trace>, reply exactly ROUNDTRIP_COMPLETE <trace>. Do not claim completion without that incoming envelope. Never send to SpecialistTwo. Repeat only for a fresh RUN.",
            "SpecialistOne": "Initially reply READY. When an external envelope from Coordinator contains START <trace>, use send_message to SpecialistTwo with body FORWARD <trace>, then reply SENT and finish. When an envelope from SpecialistTwo contains S2_ACK <trace>, use send_message to Coordinator with body S1_RELAY S2_ACK <trace>, then reply SENT and finish. No other message-tool calls. Do not invent the trace or receipt.",
            "SpecialistTwo": "Initially reply READY. When an external envelope from SpecialistOne contains FORWARD <trace>, use send_message to SpecialistOne with body S2_ACK <trace>, then reply SENT and finish. No other message-tool calls. Learn the trace only from the received peer envelope. Never send directly to Coordinator."}
        for name, agent in agents.items():
            agent.prompt(instructions[name])
        until(lambda: all(not a.busy for a in agents.values()))
        time.sleep(args.start_delay)
        for number in range(args.rounds):
            trace = "crewroom-" + uuid.uuid4().hex[:16]
            started = time.monotonic()
            snapshot["status"] = f"Round {number + 1}/{args.rounds}: live external messaging"
            agents["Coordinator"].prompt("RUN " + trace)
            until(lambda: trace in finals and all(not a.busy for a in agents.values()) and not pending)
            sends = [s for s in report["sends"] if trace in s["body"]]
            expected = [("Coordinator", "SpecialistOne", "START " + trace),
                        ("SpecialistOne", "SpecialistTwo", "FORWARD " + trace),
                        ("SpecialistTwo", "SpecialistOne", "S2_ACK " + trace),
                        ("SpecialistOne", "Coordinator", "S1_RELAY S2_ACK " + trace)]
            if [(s["from"], s["to"], s["body"]) for s in sends] != expected:
                raise RuntimeError("Protocol writes did not match the four required hops")
            item = {"trace": trace, "elapsed_seconds": round(time.monotonic() - started, 2), "passed": True}
            report["rounds"].append(item)
            record("round_complete", **item)
        if len(report["sends"]) != args.rounds * 4 or len(report["received"]) != len(report["sends"]) or len(report["acks"]) != len(report["sends"]):
            raise RuntimeError("Send/receive/ack count differs")
        for name, agent in agents.items():
            messages = agent.command("get_messages")
            (agent.root / "messages.json").write_text(json.dumps(messages, indent=2))
            if agent.command("get_subagents")["subagents"]:
                raise RuntimeError("Hidden native subagents exist")
            for trace_record in report["rounds"]:
                trace = trace_record["trace"]
                if name == "SpecialistTwo":
                    first = next(m for m in messages["messages"] if trace in message_text(m))
                    incoming = json.loads(message_text(first))
                    if first.get("role") != "user" or incoming.get("from") != "SpecialistOne":
                        raise RuntimeError("Trace leaked to SpecialistTwo before peer message")
        histories = {}
        for send in report["sends"]:
            key = send["conversation_id"]
            if key not in histories:
                histories[key] = backend.history(send["from"], key)
        (root / "mailbox-history.json").write_text(json.dumps(histories, indent=2))
        history_text = json.dumps(histories)
        if not all(s["message_id"] in history_text and s["body"] in history_text for s in report["sends"]):
            raise RuntimeError("External history is missing a protocol message")
        backend.observe()
        (root / "observer-events.json").write_text(json.dumps(backend.observer_events, indent=2))
        observer_text = json.dumps(backend.observer_events)
        if not all(s["message_id"] in observer_text and s["body"] in observer_text for s in report["sends"]):
            raise RuntimeError("Independent passive observer missed a protocol message")
        report["all_messages_visible_to_independent_observer"] = True
        report["passed"] = True
        snapshot["status"] = f"Passed: {args.rounds} rounds, {len(report['sends'])} verified hops; shutting down agents"
    except Exception as exc:
        report["error"] = f"{type(exc).__name__}: {exc}"
        snapshot["status"] = "Failed: " + report["error"]
        record("failure", error=report["error"])
    finally:
        for name, agent in agents.items():
            try:
                report["shutdown"][name] = agent.close()
                snapshot["agents"].setdefault(name, {"activity": []})["state"] = "stopped"
            except Exception as exc:
                report["shutdown"][name] = str(exc)
                report["passed"] = False
        if any(code != 0 for code in report["shutdown"].values()):
            report["passed"] = False
        if report["passed"]:
            snapshot["status"] = f"Passed: {args.rounds} rounds, {len(report['sends'])} verified hops; all agent processes stopped"
        report["finished_at"] = now()
        (root / "observer-events.json").write_text(json.dumps(backend.observer_events, indent=2))
        report["monitor_url"] = monitor_url
        (root / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        (root / "monitor.html").write_text(HTML.replace("__SNAPSHOT__", json.dumps(snapshot).replace("</", "<\\/")))
        audit.close()
        print(json.dumps({"passed": report["passed"], "report": str(root / "report.json"), "error": report.get("error")}), flush=True)
        time.sleep(args.view_seconds)
        monitor.shutdown()
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
