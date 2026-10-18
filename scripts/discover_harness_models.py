#!/usr/bin/env python3
"""Discover advertised models from installed harnesses without starting agent turns.

This contract probe is not a production execution adapter. The output intentionally
does not assert account entitlement, verified tool support, or execution readiness.
Only whitelisted model metadata is returned; raw initialization and stderr stay private.
"""
import argparse
from datetime import datetime, timezone
import json
import math
import os
from pathlib import Path
import re
import selectors
import shutil
import signal
import subprocess
import tempfile
import time
from urllib.parse import quote

MAX_OUTPUT = 8 * 1024 * 1024
MAX_MODELS = 10000
HARNESS_NAMES = ("codex", "claude-code", "omp")


class DiscoveryError(Exception):
    """An intentionally credential-free diagnostic."""


class Process:
    """Bounded metadata exchange with a disposable child, never an inference API."""

    def __init__(self, argv, cwd, timeout, env=None):
        self.deadline = time.monotonic() + timeout
        self.buffer = b""
        self.size = 0
        self.proc = subprocess.Popen(
            argv, cwd=cwd, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, start_new_session=True,
        )
        self.selector = selectors.DefaultSelector()
        self.selector.register(self.proc.stdout, selectors.EVENT_READ)

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()

    def send(self, value):
        try:
            self.proc.stdin.write((json.dumps(value) + "\n").encode())
            self.proc.stdin.flush()
        except (BrokenPipeError, OSError):
            raise DiscoveryError("Harness closed its discovery input.") from None

    def chunk(self):
        remaining = self.deadline - time.monotonic()
        if remaining <= 0 or not self.selector.select(remaining):
            raise DiscoveryError("Model discovery timed out.")
        chunk = os.read(self.proc.stdout.fileno(), 65536)
        self.size += len(chunk)
        if self.size > MAX_OUTPUT:
            raise DiscoveryError("Model discovery exceeded its output limit.")
        return chunk

    def receive(self, matches):
        while True:
            if time.monotonic() >= self.deadline:
                raise DiscoveryError("Model discovery timed out.")
            if b"\n" not in self.buffer:
                chunk = self.chunk()
                if not chunk:
                    raise DiscoveryError("Harness exited before its discovery response.")
                self.buffer += chunk
                continue
            line, self.buffer = self.buffer.split(b"\n", 1)
            if not line.strip():
                continue
            value = parse_json(line)
            if isinstance(value, dict) and matches(value):
                return value

    def collect(self):
        self.proc.stdin.close()
        data = bytearray()
        while chunk := self.chunk():
            data.extend(chunk)
        status = self.wait_unreaped(max(.01, self.deadline - time.monotonic()))
        if status is None:
            raise DiscoveryError("Harness did not exit after discovery output.")
        if status.si_code != os.CLD_EXITED or status.si_status != 0:
            raise DiscoveryError("Harness rejected the discovery command.")
        return bytes(data)

    def wait_unreaped(self, timeout):
        # Keep our child PID reserved until its process group has been stopped.
        # Reaping the leader first could leave descendants or allow PID reuse.
        until = time.monotonic() + timeout
        while True:
            status = os.waitid(os.P_PID, self.proc.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
            if status is not None or time.monotonic() >= until:
                return status
            time.sleep(min(.01, max(0, until - time.monotonic())))

    def close(self):
        try:
            if not self.proc.stdin.closed:
                try:
                    self.proc.stdin.close()
                except BrokenPipeError:
                    pass
            if self.wait_unreaped(.5) is None:
                try:
                    os.killpg(self.proc.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                self.wait_unreaped(.5)
            # The unreaped leader proves this group identity is still ours.
            # Discovery never admits work; stop any remaining helpers as well.
            try:
                os.killpg(self.proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            self.proc.wait(timeout=2)
        finally:
            self.selector.close()
            self.proc.stdout.close()


def parse_json(data):
    try:
        return json.loads(data)
    except (ValueError, UnicodeDecodeError):
        raise DiscoveryError("Harness returned invalid discovery JSON.") from None


def text(value):
    if not isinstance(value, str) or not value.strip() or len(value) > 1000:
        raise DiscoveryError("Harness returned invalid model metadata.")
    return value


def strings(value):
    if value is None:
        return None
    if not isinstance(value, list) or len(value) > 100:
        raise DiscoveryError("Harness returned invalid model capabilities.")
    return list(dict.fromkeys(text(item) for item in value))


def rows(value):
    if not isinstance(value, list) or len(value) > MAX_MODELS:
        raise DiscoveryError("Harness returned an invalid model list.")
    if any(not isinstance(row, dict) for row in value):
        raise DiscoveryError("Harness returned an invalid model entry.")
    return value


def model(harness, selector, label, *, provider=None, resolved=None,
          efforts=None, inputs=None, default_effort=None):
    selector, label = text(selector), text(label)
    return {
        # Internal offer identity, not the user-facing model name. Never merge
        # similarly named models or aliases across different harnesses implicitly.
        "key": f"{harness}:{quote(selector, safe='')}",
        "harness": harness, "selector": selector, "displayName": label,
        "provider": text(provider) if provider is not None else None,
        "resolvedModel": text(resolved) if resolved is not None else None,
        "reasoningEfforts": strings(efforts), "inputModalities": strings(inputs),
        "defaultReasoningEffort": text(default_effort) if default_effort is not None else None,
        "access": "not_verified", "toolSupport": "not_verified",
    }


def normalize(harness, entries):
    result = []
    for row in rows(entries):
        if harness == "codex":
            if row.get("hidden") is True:
                continue
            efforts = row.get("supportedReasoningEfforts")
            if efforts is not None:
                efforts = [entry.get("reasoningEffort") for entry in rows(efforts)]
            result.append(model(harness, row.get("model"), row.get("displayName"),
                                efforts=efforts, inputs=row.get("inputModalities"),
                                default_effort=row.get("defaultReasoningEffort")))
        elif harness == "claude-code":
            result.append(model(harness, row.get("value"), row.get("displayName"),
                                resolved=row.get("resolvedModel"),
                                efforts=row.get("supportedEffortLevels")))
        elif harness == "omp":
            provider, ident = text(row.get("provider")), text(row.get("id"))
            selector = f"{provider}/{ident}"
            if row.get("selector") != selector:
                raise DiscoveryError("OMP returned inconsistent model identity.")
            result.append(model(harness, selector, row.get("name"), provider=provider,
                                resolved=ident, efforts=row.get("thinking"),
                                inputs=row.get("input")))
        else:
            raise DiscoveryError("Unknown harness.")
    seen = set()
    for item in result:
        if item["key"] in seen:
            raise DiscoveryError("Harness returned duplicate model selectors.")
        seen.add(item["key"])
    return result


def codex_models(binary, cwd, timeout):
    with Process([binary, "app-server", "--listen", "stdio://"], cwd, timeout) as rpc:
        def request(ident, method, params):
            rpc.send({"id": ident, "method": method, "params": params})
            response = rpc.receive(lambda v: v.get("id") == ident)
            if "error" in response or not isinstance(response.get("result"), dict):
                raise DiscoveryError("Codex rejected a discovery request.")
            return response["result"]

        request(1, "initialize", {"clientInfo": {
            "name": "crewroom_model_discovery", "title": "Crewroom model discovery",
            "version": "0.1.0",
        }})
        rpc.send({"method": "initialized", "params": {}})
        all_models, cursors, cursor = [], set(), None
        for index in range(100):
            params = {"limit": 100, "includeHidden": False}
            if cursor is not None:
                params["cursor"] = cursor
            page = request(index + 2, "model/list", params)
            all_models.extend(rows(page.get("data")))
            if len(all_models) > MAX_MODELS:
                raise DiscoveryError("Model discovery exceeded its model limit.")
            cursor = page.get("nextCursor")
            if cursor is None:
                return normalize("codex", all_models)
            cursor = text(cursor)
            if cursor in cursors:
                raise DiscoveryError("Codex repeated a model-list cursor.")
            cursors.add(cursor)
        raise DiscoveryError("Codex exceeded its model-list page limit.")


def claude_models(binary, cwd, timeout):
    env = {key: os.environ[key] for key in ("PATH", "HOME", "LANG", "TMPDIR") if key in os.environ}
    env.update(CLAUDE_CONFIG_DIR=str(Path(cwd) / "claude-config"),
               CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC="1")
    argv = [binary, "--bare", "-p", "--input-format", "stream-json", "--output-format",
            "stream-json", "--verbose", "--no-session-persistence", "--tools", "",
            "--strict-mcp-config", "--mcp-config", '{"mcpServers":{}}',
            "--setting-sources", "", "--settings", '{"disableAllHooks":true}']
    with Process(argv, cwd, timeout, env) as rpc:
        rpc.send({"type": "control_request", "request_id": "crewroom-discovery",
                  "request": {"subtype": "initialize", "hooks": None}})
        response = rpc.receive(lambda v: v.get("type") == "control_response"
                               and isinstance(v.get("response"), dict)
                               and v["response"].get("request_id") == "crewroom-discovery")
        envelope = response["response"]
        if envelope.get("subtype") != "success" or not isinstance(envelope.get("response"), dict):
            raise DiscoveryError("Claude Code rejected initialization.")
        return normalize("claude-code", envelope["response"].get("models"))


def omp_models(binary, cwd, timeout):
    with Process([binary, "models", "--json", "--no-extensions"], cwd, timeout) as child:
        value = parse_json(child.collect())
    if not isinstance(value, dict):
        raise DiscoveryError("OMP returned an invalid catalog.")
    return normalize("omp", value.get("models"))


def version(binary, cwd, timeout):
    with Process([binary, "--version"], cwd, min(timeout, 5)) as child:
        raw = child.collect().decode("utf-8", errors="replace")
    match = re.search(r"\b\d+\.\d+\.\d+(?:[-+][\w.-]+)?\b", raw)
    return match.group() if match else None


def discover(harness, binary, timeout=15):
    output = {"harness": harness, "status": "unavailable", "version": None, "models": [],
              "scope": {"codex": "client_catalog", "claude-code": "isolated_client_menu",
                        "omp": "configured_auth_catalog"}[harness]}
    executable = shutil.which(binary)
    if executable is None:
        output["error"] = "Harness executable was not found."
        return output
    try:
        with tempfile.TemporaryDirectory(prefix="crewroom-models-") as cwd:
            output["version"] = version(executable, cwd, timeout)
            output["models"] = {"codex": codex_models, "claude-code": claude_models,
                                "omp": omp_models}[harness](executable, cwd, timeout)
        output["status"] = "ok"
    except DiscoveryError as exc:
        output.update(status="error", error=str(exc), models=[])
    except OSError:
        output.update(status="error", error="Could not run the harness discovery process.", models=[])
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--harness", action="append", choices=HARNESS_NAMES)
    parser.add_argument("--codex", default="codex", help="Codex executable path")
    parser.add_argument("--claude-code", default="claude", help="Claude Code executable path")
    parser.add_argument("--omp", default="omp", help="OMP executable path")
    parser.add_argument("--timeout", type=float, default=15, help="Seconds per command (1–60)")
    args = parser.parse_args()
    if not math.isfinite(args.timeout) or not 1 <= args.timeout <= 60:
        parser.error("--timeout must be between 1 and 60 seconds")
    selected = list(dict.fromkeys(args.harness or HARNESS_NAMES))
    result = [discover(h, getattr(args, h.replace("-", "_")), args.timeout) for h in selected]
    print(json.dumps({"schemaVersion": 1, "observedAt": datetime.now(timezone.utc).isoformat(),
                      "inferenceRequested": False, "executionAdaptersVerified": False,
                      "harnesses": result}, indent=2))
    return 0 if all(row["status"] == "ok" for row in result) else 1


if __name__ == "__main__":
    raise SystemExit(main())
