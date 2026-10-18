#!/usr/bin/env python3
"""FOR-7 bounded live Codex check using an existing, read-only login mount.

Explicitly consumes live harness access. Runs two turns, with one harmless
host-tool reply, in private filesystem/PID namespaces. Networking is shared
for Codex authentication/inference. No repository or ambient config is mounted.
This is not production credential-refresh, budget, or ownership integration.
"""
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import uuid

import probe_codex_runtime as fixture


def launch(root, model, catalog):
    state = root / "state"
    prefix = (f'model = "{model}"\nmodel_provider = "openai"\n'
              'model_reasoning_effort = "low"\nweb_search = "disabled"\n'
              'cli_auth_credentials_store = "file"\n'
              f'model_catalog_json = "{catalog}"\n'
              '[tools.update_plan]\nenabled = false\n'
              '[tools.experimental_request_user_input]\nenabled = false\n'
              '[agents]\nenabled = false\n')
    feature_config = fixture.config("unused", catalog).split("[features]", 1)[1]
    (state / "config.toml").write_text(prefix + "[features]" + feature_config)
    for path in [root / "home", root / "cwd"]:
        path.mkdir(exist_ok=True, mode=0o700)
    child = fixture.JsonProcess(["/tools/codex", "app-server", "--listen", "stdio://", "--strict-config"],
                                root / "cwd", {"PATH": "/usr/bin", "HOME": str(root / "home"),
                                               "CODEX_HOME": str(state), "LANG": "C.UTF-8"})
    try:
        reply = child.call("initialize", {"clientInfo": {"name": "crewroom_native_auth_probe", "version": "0.1.0"},
                                          "capabilities": {"experimentalApi": True}})
        fixture.check("result" in reply, "Initialization failed")
        child.send({"method": "initialized"})
        return child
    except Exception:
        child.stop()
        raise


def bounded_turn(child, thread, prompt, expected_reply, allow_tool):
    start_index = len(child.frames)
    ident = fixture.turn(child, thread, prompt)
    index = start_index
    tool_calls = []
    while True:
        # A 30-second idle bound plus the outer 180-second hard wall.
        event = child.wait(lambda v: ("id" in v and "method" in v)
                           or (v.get("method") == "turn/completed" and v["params"]["turn"]["id"] == ident),
                           index, seconds=30)
        index = child.frames.index(event, index) + 1
        if event.get("method") == "turn/completed":
            ended = event["params"]["turn"]
            fixture.check(ended["status"] == "completed", "Live turn did not complete: " + ended["status"])
            text = " ".join(i.get("text", "") for i in ended.get("items", []) if i.get("type") == "agentMessage")
            fixture.check(expected_reply in text, "Final answer did not contain the addressed tool result")
            fixture.check(len(tool_calls) == (1 if allow_tool else 0), "Unexpected host-tool count")
            return {"turnId": ident, "status": ended["status"], "toolCalls": tool_calls,
                    "returnedExpectedResult": True,
                    "usage": [v["params"]["tokenUsage"] for v in child.frames[start_index:]
                              if v.get("method") == "thread/tokenUsage/updated"]}
        fixture.check(allow_tool and not tool_calls, "Unexpected additional server request")
        fixture.check(event["method"] == "item/tool/call", "Unexpected native server request")
        params = event["params"]
        fixture.check(params["threadId"] == thread and params["turnId"] == ident, "Wrong addressed identity")
        fixture.check(params["tool"] == "crewroom_echo", "Unexpected tool")
        fixture.check(params["arguments"] == {"text": "LIVE_REQUEST"}, "Unexpected tool arguments")
        tool_calls.append({"rpcId": event["id"], **params})
        fixture.tool_reply(child, event, expected_reply)


def inner(args):
    root = args.output
    result = {"scope": "live native ChatGPT-authenticated Codex; no Crewroom gateway", "model": args.model,
              "effort": "low", "assertionsPassed": False, "authFileReadOnly": True,
              "networkNamespace": "shared host network", "repositoryMounted": False}
    children = []
    try:
        raw = json.loads(subprocess.check_output(["/tools/codex", "debug", "models", "--bundled"]))
        model = next(copy.deepcopy(m) for m in raw["models"] if m["slug"] == args.model)
        model.update({"apply_patch_tool_type": None, "supports_search_tool": False,
                      "experimental_supported_tools": [], "node_repl_disabled": True,
                      "tool_mode": "direct", "use_responses_lite": False})
        catalog = root / "restricted-models.json"
        fixture.save(catalog, {"models": [model]})
        child = launch(root, args.model, catalog)
        children.append(child)
        account = child.call("account/read", {"refreshToken": False})
        # Do not persist account identity, email, plan or auth material.
        result["accountType"] = account.get("result", {}).get("account", {}).get("type")
        result["accountResponseShape"] = {"replyKeys": list(account), "resultKeys": list(account.get("result", {})),
                                          "errorCode": account.get("error", {}).get("code"),
                                          "errorMessage": account.get("error", {}).get("message")}
        fixture.check(result["accountType"] == "chatgpt", "Existing ChatGPT auth was not loaded")
        started = child.call("thread/start", {"model": args.model, "modelProvider": "openai",
                             "cwd": str(root / "cwd"), "sandbox": "read-only", "approvalPolicy": "never",
                             "dynamicTools": fixture.TOOLS, "allowProviderModelFallback": False,
                             "developerInstructions": "This is a bounded compatibility check. Use only the supplied Crewroom tool."})
        fixture.check("result" in started, "Thread start failed")
        identity = started["result"]
        result["effective"] = {k: identity.get(k) for k in ["model", "modelProvider", "reasoningEffort", "approvalPolicy", "sandbox"]}
        fixture.check(identity["model"] == args.model and identity["modelProvider"] == "openai", "Frozen route changed")
        thread = identity["thread"]["id"]
        result["threadId"] = thread
        expected_reply = "LIVE_REPLY_" + uuid.uuid4().hex
        result["initial"] = bounded_turn(child, thread,
            'Call crewroom_echo exactly once with {"text":"LIVE_REQUEST"}. '
            'Then reply with exactly the tool result, no other text or actions.', expected_reply, True)
        result["firstStop"] = child.stop(abrupt=True)
        successor = launch(root, args.model, catalog)
        children.append(successor)
        resumed = successor.call("thread/resume", {"threadId": thread})
        fixture.check(resumed.get("result", {}).get("thread", {}).get("id") == thread, "Live thread resume failed")
        result["resumedSameThread"] = True
        result["afterRestart"] = bounded_turn(successor, thread,
            'Without calling any tools, repeat exactly the result returned by crewroom_echo in the previous turn.',
            expected_reply, False)
        result["modelReroutes"] = [v["params"] for process in children for v in process.frames
                                  if v.get("method") == "model/rerouted"]
        fixture.check(not result["modelReroutes"], "Model reroute was observed")
        result["assertionsPassed"] = True
    except Exception as exc:
        result["probeError"] = str(exc)
    finally:
        result["cleanup"] = [child.stop() for child in reversed(children)]
    fixture.save(root / "result.json", result)
    print(json.dumps(result, indent=2), flush=True)
    if not result["assertionsPassed"]:
        raise SystemExit(1)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--model", required=True)
    parser.add_argument("--codex", type=Path)
    parser.add_argument("--auth-file", type=Path)
    parser.add_argument("--inside", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.inside:
        inner(args)
        return
    if not args.codex or not args.auth_file:
        parser.error("Explicit pinned --codex and existing --auth-file are required")
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False, mode=0o700)
    (root / "state").mkdir(mode=0o700)
    auth = args.auth_file.resolve(strict=True)
    auth_before = auth.stat()
    script = Path(__file__).resolve()
    command = ["bwrap", "--unshare-all", "--share-net", "--die-with-parent", "--new-session",
               "--ro-bind", "/usr", "/usr", "--symlink", "usr/bin", "/bin",
               "--symlink", "usr/lib", "/lib", "--symlink", "usr/lib", "/lib64",
               "--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp", "--dir", "/etc",
               "--ro-bind", "/etc/resolv.conf", "/etc/resolv.conf",
               "--ro-bind", "/etc/ssl", "/etc/ssl",
               "--ro-bind", "/etc/ca-certificates", "/etc/ca-certificates",
               "--bind", str(root), "/work", "--ro-bind", str(auth), "/work/state/auth.json",
               "--ro-bind", str(script), "/probe_native.py",
               "--ro-bind", str(script.with_name("probe_codex_runtime.py")), "/probe_codex_runtime.py",
               "--ro-bind", str(args.codex.resolve(strict=True)), "/tools/codex",
               "--clearenv", "--setenv", "PATH", "/usr/bin", "--setenv", "LANG", "C.UTF-8",
               "--chdir", "/work", "--", "/usr/bin/python3", "/probe_native.py", "--inside",
               "--output", "/work", "--model", args.model]
    fixture.save(root / "launch.json", {"sourceRevision": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
                 "model": args.model, "codexPath": str(args.codex),
                 "scriptSha256": hashlib.sha256(script.read_bytes()).hexdigest(),
                 "helperSha256": hashlib.sha256(script.with_name("probe_codex_runtime.py").read_bytes()).hexdigest(),
                 "codexSha256": hashlib.sha256(args.codex.read_bytes()).hexdigest(),
                 "authSource": "existing Codex login, read-only bind; no credential copy", "wallLimitSeconds": 180})
    try:
        completed = subprocess.run(command, timeout=180)
    finally:
        auth_after = auth.stat()
        fixture.save(root / "auth-source-check.json", {"sizeAndMtimeUnchanged":
                     (auth_before.st_size, auth_before.st_mtime_ns) == (auth_after.st_size, auth_after.st_mtime_ns)})
    raise SystemExit(completed.returncode)


if __name__ == "__main__":
    main()
