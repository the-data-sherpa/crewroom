#!/usr/bin/env python3
"""FOR-7: isolated real Codex/synthetic provider compatibility experiment.

This does not enable a production adapter. The outer process runs the entire
experiment in a fresh Linux network/filesystem/PID namespace. Only /usr,
explicit fixture binaries/scripts, and the new output directory are mounted.
No credentials or real provider connection are admitted.
"""
import argparse
import copy
import hashlib
import http.client
import http.server
import json
import os
from pathlib import Path
import queue
import signal
import subprocess
import sys
import threading
import time
import urllib.parse
import uuid

MODEL = "gpt-6.1-sol"
ROOT = Path(__file__).resolve().parent.parent
LIMIT = 8 * 1024 * 1024


def save(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


class JsonProcess:
    def __init__(self, argv, cwd, env=None, initial=None):
        self.frames = []
        self.condition = threading.Condition()
        self.serial = 0
        self.log = (cwd / ("process-" + uuid.uuid4().hex + ".stderr")).open("wb")
        self.proc = subprocess.Popen(argv, cwd=cwd, env=env, stdin=subprocess.PIPE,
                                     stdout=subprocess.PIPE, stderr=self.log,
                                     start_new_session=True)
        self.thread = threading.Thread(target=self.read, daemon=True)
        self.thread.start()
        if initial is not None:
            self.send(initial)

    def read(self):
        try:
            while line := self.proc.stdout.readline(LIMIT + 1):
                if len(line) > LIMIT or not line.endswith(b"\n"):
                    raise RuntimeError("Invalid or oversized fixture frame")
                value = json.loads(line)
                with self.condition:
                    self.frames.append(value)
                    self.condition.notify_all()
        except Exception as exc:
            with self.condition:
                self.frames.append({"fixtureReaderError": str(exc)})
        finally:
            with self.condition:
                self.condition.notify_all()

    def send(self, value):
        self.proc.stdin.write((json.dumps(value) + "\n").encode())
        self.proc.stdin.flush()

    def wait(self, predicate, start=0, seconds=20):
        end = time.monotonic() + seconds
        with self.condition:
            while True:
                for value in self.frames[start:]:
                    if predicate(value):
                        return value
                    if "fixtureReaderError" in value:
                        raise RuntimeError(value["fixtureReaderError"])
                if self.proc.poll() is not None:
                    raise RuntimeError(f"Fixture process exited {self.proc.returncode}")
                remaining = end - time.monotonic()
                if remaining <= 0:
                    raise TimeoutError("Fixture event did not arrive")
                self.condition.wait(min(remaining, .2))

    def call(self, method, params):
        self.serial += 1
        ident = self.serial
        start = len(self.frames)
        self.send({"id": ident, "method": method, "params": params})
        return self.wait(lambda v: v.get("id") == ident and ("result" in v or "error" in v), start)

    def stop(self, abrupt=False):
        if self.proc.poll() is None:
            os.killpg(self.proc.pid, signal.SIGKILL if abrupt else signal.SIGTERM)
            try:
                self.proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(self.proc.pid, signal.SIGKILL)
                self.proc.wait(timeout=5)
        self.thread.join(2)
        self.log.close()
        return {"pid": self.proc.pid, "exitCode": self.proc.returncode, "reaped": self.proc.poll() is not None}


class HttpSurface:
    """Scripted Responses server or transparent capture in front of Rust gateway."""
    def __init__(self, forward=None):
        self.forward = forward
        self.requests = []
        self.errors = []
        self.actions = queue.Queue()
        owner = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_GET(self):
                owner.requests.append({"path": self.path, "method": "GET"})
                self.send_response(404)
                self.end_headers()

            def do_POST(self):
                try:
                    length = int(self.headers.get("Content-Length", 0))
                    if length > LIMIT:
                        raise ValueError("Oversized HTTP fixture request")
                    raw = self.rfile.read(length)
                    try:
                        body = json.loads(raw)
                    except (ValueError, UnicodeDecodeError):
                        body = None
                    row = {"path": self.path, "method": "POST", "body": body,
                           "bytes": len(raw), "sha256": hashlib.sha256(raw).hexdigest(),
                           "encoding": self.headers.get("Content-Encoding")}
                    owner.requests.append(row)
                    if owner.forward:
                        url = urllib.parse.urlsplit(owner.forward)
                        connection = http.client.HTTPConnection(url.hostname, url.port, timeout=30)
                        try:
                            connection.request("POST", self.path, raw, dict(self.headers))
                            response = connection.getresponse()
                            row["status"] = response.status
                            self.send_response(response.status)
                            self.send_header("Content-Type", response.getheader("Content-Type", "application/json"))
                            self.end_headers()
                            while chunk := response.read1(65536):
                                self.wfile.write(chunk)
                                self.wfile.flush()
                        finally:
                            connection.close()
                        return
                    action = owner.actions.get(timeout=3)
                    if action.get("hold") and not action.get("streaming"):
                        action["entered"].set()
                        action["release"].wait(15)
                    if action.get("error"):
                        self.send_response(action["error"])
                        self.send_header("Content-Type", "application/json")
                        self.end_headers()
                        self.wfile.write(b'{"error":{"message":"Synthetic fixture error"}}')
                        return
                    events = encode(action)
                    self.send_response(200)
                    self.send_header("Content-Type", "text/event-stream")
                    self.end_headers()
                    for index, event in enumerate(events):
                        self.wfile.write(("event: " + event["type"] + "\ndata: " + json.dumps(event) + "\n\n").encode())
                        self.wfile.flush()
                        if action.get("streaming") and index == 1:
                            action["entered"].set()
                            action["release"].wait(15)
                except (BrokenPipeError, ConnectionResetError):
                    pass
                except Exception as exc:
                    owner.errors.append(str(exc))
                    try:
                        self.send_error(500, "Fixture error")
                    except OSError:
                        pass

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.daemon_threads = True
        self.origin = f"http://127.0.0.1:{self.server.server_port}"
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(2)


def encode(action):
    ident = uuid.uuid4().hex
    response = {"id": "resp_" + ident, "object": "response", "status": "completed", "model": MODEL,
                "usage": {"input_tokens": 10, "output_tokens": 5, "total_tokens": 15}}
    if "custom" in action:
        item = {"type": "custom_tool_call", "id": "ctc_" + ident,
                "call_id": "call_" + ident, "name": action["custom"], "input": ""}
        done = {**item, "input": action["input"], "status": "completed"}
        delta = {"type": "response.custom_tool_call_input.delta", "item_id": item["id"], "output_index": 0,
                 "delta": action["input"]}
    elif "tool" in action:
        item = {"type": "function_call", "id": "fc_" + ident,
                "call_id": action.get("callId", "call_" + ident), "name": action["tool"], "arguments": ""}
        done = {**item, "arguments": json.dumps(action.get("arguments", {})), "status": "completed"}
        delta = {"type": "response.function_call_arguments.delta", "item_id": item["id"], "output_index": 0,
                 "delta": done["arguments"]}
    else:
        item = {"type": "message", "id": "msg_" + ident, "role": "assistant", "content": [], "status": "in_progress"}
        done = {**item, "status": "completed", "content": [{"type": "output_text", "text": action.get("text", "FIXTURE_OK"), "annotations": []}]}
        delta = {"type": "response.output_text.delta", "item_id": item["id"], "output_index": 0,
                 "content_index": 0, "delta": action.get("text", "FIXTURE_OK")}
    return [{"type": "response.created", "response": {**response, "status": "in_progress", "output": []}},
            {"type": "response.output_item.added", "output_index": 0, "item": item}, delta,
            {"type": "response.output_item.done", "output_index": 0, "item": done},
            {"type": "response.completed", "response": {**response, "output": [done]}}]


TOOLS = [{"type": "function", "name": "crewroom_echo", "description": "Return the addressed fixture value.",
          "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}},
                          "required": ["text"], "additionalProperties": False}}]


def config(base_url, catalog=None):
    prefix = f'model = "{MODEL}"\nmodel_provider = "crewroom_probe"\nmodel_reasoning_effort = "low"\nweb_search = "disabled"\n'
    if catalog:
        prefix += "model_catalog_json = " + json.dumps(str(catalog)) + "\n"
    if catalog:
        prefix += '[tools.update_plan]\nenabled = false\n[tools.experimental_request_user_input]\nenabled = false\n[agents]\nenabled = false\n'
    value = prefix + f'''[model_providers.crewroom_probe]
name = "Crewroom isolated synthetic provider"
base_url = "{base_url}"
env_key = "CREWROOM_PROBE_TOKEN"
wire_api = "responses"
requires_openai_auth = false
supports_websockets = false
request_max_retries = 0
stream_max_retries = 0
[features]
shell_tool = false
unified_exec = false
shell_snapshot = false
multi_agent = false
apps = false
plugins = false
hooks = false
code_mode_host = false
image_generation = false
browser_use = false
computer_use = false
view_image = false
'''
    if catalog:
        value += '''multi_agent_v2 = false
code_mode = false
code_mode_only = false
code_mode_tool_search = false
skill_search = false
goals = false
sleep_tool = false
tool_suggest = false
enable_request_compression = false
'''
    return value


def codex(root, origin, catalog=None, session_root=None):
    state = session_root or root / "state"
    for path in [root / "home", root / "cwd", state]:
        path.mkdir(parents=True, exist_ok=True, mode=0o700)
    (state / "config.toml").write_text(config(origin, catalog))
    env = {"PATH": "/usr/bin", "HOME": str(root / "home"), "CODEX_HOME": str(state),
           "CREWROOM_PROBE_TOKEN": "host-token", "LANG": "C.UTF-8"}
    child = JsonProcess(["/tools/codex", "app-server", "--listen", "stdio://", "--strict-config"], root / "cwd", env)
    try:
        reply = child.call("initialize", {"clientInfo": {"name": "crewroom_runtime_probe", "version": "0.1.0"},
                                          "capabilities": {"experimentalApi": True}})
        assert "result" in reply, reply
        child.send({"method": "initialized"})
    except Exception:
        child.stop()
        raise
    return child


def start(child, root):
    value = child.call("thread/start", {"model": MODEL, "modelProvider": "crewroom_probe", "cwd": str(root / "cwd"),
                                      "sandbox": "read-only", "approvalPolicy": "never", "dynamicTools": TOOLS,
                                      "allowProviderModelFallback": False,
                                      "developerInstructions": "Isolated compatibility fixture. Use only registered Crewroom tools."})
    assert "result" in value, value
    return value["result"]


def turn(child, thread, message="Run the isolated compatibility fixture."):
    value = child.call("turn/start", {"threadId": thread, "input": [{"type": "text", "text": message}]})
    assert "result" in value, value
    return value["result"]["turn"]["id"]


def terminal(child, ident, start=0):
    return child.wait(lambda v: v.get("method") == "turn/completed" and v["params"]["turn"]["id"] == ident,
                      start, seconds=30)["params"]["turn"]


def inventory(body):
    if not isinstance(body, dict):
        return None
    return [{k: item[k] for k in ["type", "name"] if k in item} for item in body.get("tools", [])]


def gateway(root, upstream, context_window=2000000):
    root.mkdir(mode=0o700)
    plan = {"resolved": {"selector": "crewroom_probe/" + MODEL, "id": MODEL,
                         "api": "openai-responses", "provider": "crewroom_probe", "effort": "low"}}
    provider = {"baseUrl": upstream + "/v1", "apiKey": "synthetic-upstream-key", "requestTimeoutMs": 20000,
                "models": [{"id": MODEL, "contextWindow": context_window, "maxTokens": 1000}]}
    child = JsonProcess(["/tools/gateway"], root, initial={"root": str(root), "plan": plan, "provider": provider})
    origin = child.wait(lambda v: v.get("ready") is True)["origin"]
    return child, origin


def run_case(root, name, catalog=None, through_gateway=True):
    root = root / name
    root.mkdir(mode=0o700)
    upstream = HttpSurface()
    children = []
    capture = None
    result = {"name": name, "realRunner": True, "provider": "synthetic", "throughRustGateway": through_gateway}
    try:
        if through_gateway:
            guard, origin = gateway(root / "gateway", upstream.origin)
            children.append(guard)
            capture = HttpSurface(origin)
            provider_url = capture.origin + "/provider/v1"
        else:
            provider_url = upstream.origin + "/v1"
        upstream.actions.put({"text": "FIXTURE_OK"})
        agent = codex(root, provider_url, catalog)
        children.append(agent)
        identity = start(agent, root)
        ident = turn(agent, identity["thread"]["id"])
        result["terminal"] = terminal(agent, ident)
        source = capture.requests if capture else upstream.requests
        result["requests"] = [{"path": row["path"], "status": row.get("status"), "encoding": row.get("encoding"),
                               "tools": inventory(row.get("body")), "bodyKeys": sorted((row.get("body") or {}).keys())}
                              for row in source]
        result["upstreamRequests"] = len(upstream.requests)
        if through_gateway:
            result["gatewayEvents"] = [v["event"] for v in guard.frames if "event" in v]
        result["threadId"] = identity["thread"]["id"]
        result["resume"] = agent.call("thread/resume", {"threadId": result["threadId"]})
        save(root / "requests.json", source)
        save(root / "frames.json", agent.frames)
        result["fixtureErrors"] = upstream.errors + (capture.errors if capture else [])
        if name == "flags-only-gateway":
            check(result["terminal"]["status"] == "failed" and result["upstreamRequests"] == 0,
                  "Baseline unsupported input should be denied")
        else:
            check(result["terminal"]["status"] == "completed", "Restricted profile did not complete")
            check(all(row["tools"] == [{"type": "function", "name": "crewroom_echo"}] for row in result["requests"]),
                  "Unexpected tool inventory")
        result["assertionsPassed"] = True
    except Exception as exc:
        result["probeError"] = str(exc)
    finally:
        result["cleanup"] = [child.stop() for child in reversed(children)]
        if capture:
            capture.close()
        upstream.close()
    save(root / "result.json", result)
    return result


def check(value, message):
    if not value:
        raise AssertionError(message)


def tool_reply(child, request, text="ADDRESSED_TOOL_RESULT"):
    child.send({"id": request["id"], "result": {"success": True,
                "contentItems": [{"type": "inputText", "text": text}]}})


def tool_request(child, thread, turn_id, start_index=0):
    request = child.wait(lambda v: v.get("method") == "item/tool/call", start_index)
    check(request["params"]["threadId"] == thread, "Wrong addressed thread")
    check(request["params"]["turnId"] == turn_id, "Wrong addressed turn")
    check(request["params"]["tool"] == "crewroom_echo", "Unregistered tool reached host")
    return request


def exercise(root, name, catalog, kind):
    root = root / name
    root.mkdir(mode=0o700)
    upstream = HttpSurface()
    children = []
    capture = None
    result = {"name": name, "kind": kind, "provider": "synthetic", "assertionsPassed": False}
    release = threading.Event()
    try:
        guard, origin = gateway(root / "gateway", upstream.origin,
                                context_window=60000 if kind == "policy-context" else 2000000)
        children.append(guard)
        capture = HttpSurface(origin)
        provider_url = capture.origin + "/provider/v1"
        agent = codex(root, provider_url, catalog)
        children.append(agent)
        identity = start(agent, root)
        thread = identity["thread"]["id"]
        result["threadId"] = thread
        if kind == "roundtrip":
            upstream.actions.put({"tool": "crewroom_echo", "arguments": {"text": "REQUEST_MARKER"}})
            upstream.actions.put({"text": "TOOL_FINISHED"})
            ident = turn(agent, thread)
            request = tool_request(agent, thread, ident)
            result["serverRequest"] = request
            tool_reply(agent, request)
            result["terminal"] = terminal(agent, ident)
            check(result["terminal"]["status"] == "completed", "Tool turn failed")
            check(len(upstream.requests) == 2, "Expected two model exchanges")
            outputs = [v for v in upstream.requests[-1]["body"]["input"] if v.get("type") == "function_call_output"]
            check(any(v["call_id"] == request["params"]["callId"] and "ADDRESSED_TOOL_RESULT" in json.dumps(v) for v in outputs), "Addressed result missing")
            result["addressedResultObserved"] = True
        elif kind.startswith("deny:"):
            tool = kind.split(":", 1)[1]
            marker = root / "NATIVE_ACTION_OCCURRED"
            patch = "*** Begin Patch\n*** Add File: " + str(marker) + "\n+unexpected\n*** End Patch"
            if tool == "apply_patch_custom":
                action = {"custom": "apply_patch", "input": patch}
            else:
                action = {"tool": tool, "arguments": {"command": "touch " + str(marker), "cmd": "touch " + str(marker),
                                                        "patch": patch, "message": "Return fixture text", "url": "http://127.0.0.1:9"}}
            upstream.actions.put(action)
            upstream.actions.put({"text": "AFTER_DENIAL"})
            ident = turn(agent, thread)
            result["terminal"] = terminal(agent, ident)
            check(not marker.exists(), "Native mutation bypassed host tools")
            check(not any(v.get("method") == "item/tool/call" for v in agent.frames), "Unknown call forwarded to host")
            outputs = [v for row in capture.requests[1:] for v in row.get("body", {}).get("input", [])
                       if v.get("type") in ["function_call_output", "custom_tool_call_output"]]
            result["toolOutputs"] = outputs
            result["nativeMarkerAbsent"] = True
            result["serverToolCalls"] = 0
            check(any("unsupported" in json.dumps(v).lower() for v in outputs), "No explicit unsupported-tool denial observed")
        elif kind == "interrupt-stream":
            entered = threading.Event()
            upstream.actions.put({"text": "STREAM_FINISHED", "streaming": True, "entered": entered, "release": release})
            ident = turn(agent, thread)
            check(entered.wait(10), "Streaming never began")
            result["interruptReply"] = agent.call("turn/interrupt", {"threadId": thread, "turnId": ident})
            result["terminal"] = terminal(agent, ident)
            check(result["terminal"]["status"] == "interrupted", "Interrupt looked like successful completion")
            release.set()
        elif kind == "interrupt-tool":
            upstream.actions.put({"tool": "crewroom_echo", "arguments": {"text": "WAIT"}})
            ident = turn(agent, thread)
            result["serverRequest"] = tool_request(agent, thread, ident)
            result["interruptReply"] = agent.call("turn/interrupt", {"threadId": thread, "turnId": ident})
            result["terminal"] = terminal(agent, ident)
            check(result["terminal"]["status"] == "interrupted", "Tool interruption not terminal")
        elif kind == "resume-completed":
            upstream.actions.put({"text": "BEFORE_RESTART"})
            ident = turn(agent, thread)
            check(terminal(agent, ident)["status"] == "completed", "Initial turn failed")
            result["firstStop"] = agent.stop(abrupt=True)
            successor = codex(root, provider_url, catalog)
            children.append(successor)
            resumed = successor.call("thread/resume", {"threadId": thread})
            result["resume"] = resumed
            check("result" in resumed and resumed["result"]["thread"]["id"] == thread, "Thread recovery failed")
            upstream.actions.put({"tool": "crewroom_echo", "arguments": {"text": "AFTER_RESTART"}})
            upstream.actions.put({"text": "RECOVERY_FINISHED"})
            ident = turn(successor, thread)
            request = tool_request(successor, thread, ident)
            tool_reply(successor, request, "RECOVERED_TOOL_RESULT")
            result["terminal"] = terminal(successor, ident)
            check(result["terminal"]["status"] == "completed", "Recovered turn failed")
            check("BEFORE_RESTART" in json.dumps(upstream.requests[-2]["body"]), "Prior history absent")
            result["historyAndDynamicToolSurvived"] = True
        elif kind == "policy-effort":
            result["turnReply"] = agent.call("turn/start", {"threadId": thread, "effort": "high",
                    "input": [{"type": "text", "text": "Attempt a deliberately unadmitted effort."}]})
            ident = result["turnReply"]["result"]["turn"]["id"]
            result["terminal"] = terminal(agent, ident)
            check(result["terminal"]["status"] == "failed" and not upstream.requests, "Effort substitution reached upstream")
        elif kind == "policy-context":
            huge = "bounded fixture content " * 4000
            ident = turn(agent, thread, huge)
            result["terminal"] = terminal(agent, ident)
            check(result["terminal"]["status"] == "failed" and not upstream.requests, "Oversized context reached upstream")
            check(any(v.get("event", {}).get("cause") == "context_window" for v in guard.frames),
                  "Context rejection did not reach gateway budget policy")
        elif kind == "provider-error":
            upstream.actions.put({"error": 429})
            ident = turn(agent, thread)
            result["terminal"] = terminal(agent, ident)
            check(result["terminal"]["status"] == "failed" and len(upstream.requests) == 1, "Provider failure was not bounded")
            check(upstream.requests[0]["body"]["model"] == MODEL, "Unexpected model route")
        elif kind == "compact":
            upstream.actions.put({"text": "COMPACTION_BASELINE"})
            ident = turn(agent, thread)
            check(terminal(agent, ident)["status"] == "completed", "Compaction baseline failed")
            before = len(capture.requests)
            frame_index = len(agent.frames)
            upstream.actions.put({"text": "COMPACTION_SUMMARY"})
            result["compactReply"] = agent.call("thread/compact/start", {"threadId": thread})
            compact_started = agent.wait(lambda v: v.get("method") == "turn/started", frame_index)
            compact_id = compact_started["params"]["turn"]["id"]
            result["compactionTerminal"] = terminal(agent, compact_id, frame_index)
            check(result["compactionTerminal"]["status"] == "completed", "Compaction turn failed")
            check(len(capture.requests) == before + 1, "Unexpected compaction request count")
            result["compactionRequest"] = {k: capture.requests[before].get(k) for k in ["path", "status", "encoding"]}
            check(result["compactionRequest"]["status"] == 200, "Compaction request rejected")
            upstream.actions.put({"tool": "crewroom_echo", "arguments": {"text": "AFTER_COMPACTION"}})
            upstream.actions.put({"text": "COMPACTED_TOOL_FINISHED"})
            frame_index = len(agent.frames)
            ident = turn(agent, thread)
            request = tool_request(agent, thread, ident, frame_index)
            tool_reply(agent, request)
            result["terminal"] = terminal(agent, ident)
            check(result["terminal"]["status"] == "completed", "Post-compaction tool turn failed")
            check("COMPACTION_SUMMARY" in json.dumps(upstream.requests[-2]["body"]), "Compaction summary absent")
            result["compactionCompatible"] = True
        elif kind == "two-processes":
            peer_root = root / "peer"
            peer = codex(peer_root, provider_url, catalog)
            children.append(peer)
            peer_identity = start(peer, peer_root)
            check(agent.proc.pid != peer.proc.pid, "Agents share process identity")
            check(peer_identity["thread"]["id"] != thread, "Agents share session identity")
            result["runnerPids"] = [agent.proc.pid, peer.proc.pid]
            result["threadIds"] = [thread, peer_identity["thread"]["id"]]
            for current, tid in [(agent, thread), (peer, peer_identity["thread"]["id"])]:
                upstream.actions.put({"tool": "crewroom_echo", "arguments": {"text": tid}})
                upstream.actions.put({"text": "ADDRESSED_AGENT_FINISHED"})
                ident = turn(current, tid)
                request = tool_request(current, tid, ident)
                check(request["params"]["arguments"]["text"] == tid, "Wrong addressed arguments")
                tool_reply(current, request, tid)
                check(terminal(current, ident)["status"] == "completed", "Concurrent-process assignment failed")
            result["distinctAddressedProcesses"] = True
        elif kind.startswith("crash:"):
            point = kind.split(":", 1)[1]
            # Establish durable history before the ambiguous operation.
            upstream.actions.put({"text": "DURABLE_BASELINE"})
            first = turn(agent, thread)
            check(terminal(agent, first)["status"] == "completed", "Baseline turn failed")
            start_index = len(agent.frames)
            upstream.actions.put({"tool": "crewroom_echo", "callId": "stable_fixture_call", "arguments": {"text": point}})
            ident = turn(agent, thread)
            request = tool_request(agent, thread, ident, start_index)
            result["originalRequest"] = request
            # One observable stand-in effect, not a claim about production ledgers.
            if point != "before-effect":
                with (root / "effect-count.txt").open("x") as file:
                    file.write("1\n")
                    file.flush()
                    os.fsync(file.fileno())
            if point in ["after-receipt", "after-reply"]:
                with (root / "host-receipt.json").open("x") as file:
                    json.dump({"callId": request["params"]["callId"], "result": "SAVED_RESULT"}, file)
                    file.flush()
                    os.fsync(file.fileno())
            if point == "after-reply":
                entered = threading.Event()
                upstream.actions.put({"text": "LATE_RESULT", "hold": True, "entered": entered, "release": release})
                tool_reply(agent, request, "SAVED_RESULT")
                check(entered.wait(10), "Tool result never reached provider request")
            result["firstStop"] = agent.stop(abrupt=True)
            release.set()
            successor = codex(root, provider_url, catalog)
            children.append(successor)
            result["resume"] = successor.call("thread/resume", {"threadId": thread})
            check("result" in result["resume"], "Crashed thread not resumable")
            check(result["resume"]["result"]["thread"]["id"] == thread, "Wrong resumed thread")
            upstream.actions.put({"text": "RECONCILIATION_PROBE"})
            resumed_turn = turn(successor, thread, "Report retained fixture history without invoking a tool.")
            result["terminal"] = terminal(successor, resumed_turn)
            result["replayedServerRequests"] = [v for v in successor.frames if v.get("method") == "item/tool/call"]
            result["effectCount"] = int((root / "effect-count.txt").read_text()) if (root / "effect-count.txt").exists() else 0
            result["hostReceiptPresent"] = (root / "host-receipt.json").exists()
            result["recoveryRequestInput"] = upstream.requests[-1].get("body", {}).get("input")
            check(result["effectCount"] == (0 if point == "before-effect" else 1), "Probe duplicated side effect")
            outputs = [v for v in result["recoveryRequestInput"] if v.get("type") == "function_call_output"
                       and v.get("call_id") == "stable_fixture_call"]
            result["recoveredToolOutputs"] = outputs
            expected = "SAVED_RESULT" if point == "after-reply" else "aborted"
            check(any(v.get("output") == expected for v in outputs), "Unexpected retained tool outcome")
            check(not result["replayedServerRequests"], "Unexpected automatic tool replay")
            # No exactly-once guarantee is inferred from absent replay.
        else:
            raise ValueError(kind)
        result["assertionsPassed"] = True
    except Exception as exc:
        result["probeError"] = str(exc)
    finally:
        release.set()
        result["cleanup"] = [child.stop() for child in reversed(children)]
        result["gatewayEvents"] = [v["event"] for v in children[0].frames if "event" in v] if children else []
        result["requests"] = capture.requests if capture else []
        result["upstreamRequests"] = upstream.requests
        result["fixtureErrors"] = upstream.errors + (capture.errors if capture else [])
        if result["fixtureErrors"]:
            result["assertionsPassed"] = False
        for i, child in enumerate(children):
            save(root / f"frames-{i}.json", child.frames)
        if capture:
            capture.close()
        upstream.close()
    save(root / "result.json", result)
    print(json.dumps({"case": name, "passed": result["assertionsPassed"], "error": result.get("probeError")}), flush=True)
    return result


def inner(args):
    root = Path(args.output)
    save(root / "isolation.json", {"homeMounted": Path("/home").exists(),
                                  "interfaces": json.loads(subprocess.check_output(["ip", "-j", "address"])),
                                  "routes": json.loads(subprocess.check_output(["ip", "-j", "route"]))})
    raw = json.loads(subprocess.check_output(["/tools/codex", "debug", "models", "--bundled"]))
    model = next(copy.deepcopy(m) for m in raw["models"] if m["slug"] == MODEL)
    model.update({"apply_patch_tool_type": None, "supports_search_tool": False,
                  "experimental_supported_tools": [], "node_repl_disabled": True})
    model["tool_mode"] = "direct"
    model["use_responses_lite"] = False
    catalog = root / "restricted-models.json"
    save(catalog, {"models": [model]})
    results = [run_case(root, "flags-only-gateway"),
               run_case(root, "restricted-gateway", catalog),
               run_case(root, "restricted-direct-diagnostic", catalog, False)]
    for kind in ["roundtrip", "deny:exec_command", "deny:apply_patch", "deny:apply_patch_custom", "deny:spawn_agent",
                 "deny:update_plan", "deny:request_user_input", "deny:web.run", "interrupt-stream", "interrupt-tool",
                 "resume-completed", "crash:before-effect", "crash:after-effect", "crash:after-receipt", "crash:after-reply",
                 "policy-effort", "policy-context", "provider-error", "compact", "two-processes"]:
        results.append(exercise(root, kind.replace(":", "-"), catalog, kind))
    save(root / "result.json", {"scope": "real Codex, synthetic Responses only", "cases": results})
    print(json.dumps({"cases": [{"name": r["name"], "passed": r.get("assertionsPassed"), "error": r.get("probeError"),
                                 "terminal": r.get("terminal", {}).get("status")} for r in results]}, indent=2))
    if not all(r.get("assertionsPassed") is True for r in results):
        raise SystemExit(1)


def main():
    global MODEL
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--codex", type=Path)
    parser.add_argument("--model", default=MODEL)
    parser.add_argument("--inside", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    MODEL = args.model
    if args.inside:
        inner(args)
        return
    if not args.codex:
        parser.error("--codex must name the pinned executable")
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False, mode=0o700)
    executable = args.codex.resolve(strict=True)
    binaries = {"codex": executable, "gateway": ROOT / "target/debug/examples/gateway-fixture",
                "oracle": ROOT / "target/debug/examples/provider-oracle"}
    command = ["bwrap", "--unshare-all", "--die-with-parent", "--new-session",
               "--ro-bind", "/usr", "/usr", "--symlink", "usr/bin", "/bin",
               "--symlink", "usr/lib", "/lib", "--symlink", "usr/lib", "/lib64",
               "--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp", "--dir", "/etc",
               "--bind", str(root), "/work", "--ro-bind", str(Path(__file__).resolve()), "/probe.py",
               "--clearenv", "--setenv", "PATH", "/usr/bin", "--setenv", "LANG", "C.UTF-8"]
    for name, path in binaries.items():
        command += ["--ro-bind", str(path), "/tools/" + name]
    command += ["--chdir", "/work", "--", "/usr/bin/python3", "/probe.py", "--inside", "--output", "/work", "--model", MODEL]
    save(root / "launch.json", {"command": command, "sourceRevision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
                               "scriptSha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                               "binaries": {name: {"path": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()} for name, path in binaries.items()}})
    completed = subprocess.run(command, timeout=600)
    raise SystemExit(completed.returncode)


if __name__ == "__main__":
    main()
