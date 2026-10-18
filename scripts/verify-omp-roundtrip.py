#!/usr/bin/env python3
"""Verify retained OMP round-trip evidence without running a model or exposing raw logs."""
import argparse
import hashlib
import json
from pathlib import Path


def json_lines(path):
    return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]


def verify(root):
    report = json.loads((root / "report.json").read_text())
    trace = report["trace"]
    frames = json_lines(root / "events.jsonl")
    events = []
    for frame in frames:
        if frame.get("type") == "subagent_event":
            events.append((frame["payload"]["id"], frame["payload"]["event"]))
        else:
            events.append(("Main", frame))
    sessions = {}
    files = {}
    for path in (root / "sessions").rglob("*.jsonl"):
        entries = json_lines(path)
        header = next((e for e in entries if e.get("type") == "session"), None)
        if header:
            name = "Main" if header["id"] == report["coordinator_session_id"] else path.stem
            if name in sessions:
                raise ValueError("Duplicate session name")
            sessions[name] = (header, entries)
            files[name] = path
    if set(sessions) != {"Main", "SpecialistOne", "SpecialistTwo"}:
        raise ValueError("Expected exactly three recorded sessions")
    if len({header["id"] for header, _ in sessions.values()}) != 3:
        raise ValueError("Session IDs are not distinct")

    starts = [(who, e) for who, e in events if e.get("type") == "tool_execution_start"]
    for who, e in starts:
        allowed = {"task", "read", "write"} if who == "Main" else {"read", "write", "yield"}
        if e["toolName"] not in allowed:
            raise ValueError("Unexpected tool executed")
        if e["toolName"] in {"read", "write"} and not e["args"]["path"].startswith("agent://"):
            raise ValueError("Non-agent read/write executed")
    spawn = [e["args"] for who, e in starts if e["toolName"] == "task"]
    if len(spawn) != 1 or len(spawn[0]["tasks"]) != 2:
        raise ValueError("Expected one creation call with two specialists")
    if trace in spawn[0].get("context", ""):
        raise ValueError("Trace leaked through shared task context")
    writes = [(who, e) for who, e in starts if e["toolName"] == "write"]
    expected = [
        ("Main", "SpecialistOne", f"START {trace}"),
        ("SpecialistOne", "SpecialistTwo", f"FORWARD {trace}"),
        ("SpecialistTwo", "SpecialistOne", f"S2_ACK {trace}"),
        ("SpecialistOne", "Main", f"S1_RELAY S2_ACK {trace}"),
    ]
    if len(writes) != 4:
        raise ValueError("Expected exactly four explicit message writes")
    hops = []
    for (who, start), (sender, recipient, expected_text) in zip(writes, expected):
        payload = start["args"].get("content", "")
        if who != sender or start["args"]["path"] != f"agent://{recipient}":
            raise ValueError("Wrong hop sender or recipient")
        if (sender == "Main" and not payload.startswith(expected_text + " ")) or (sender != "Main" and payload != expected_text):
            raise ValueError("Wrong message payload")
        ends = [e for actor, e in events if actor == sender and e.get("type") == "tool_execution_end"
                and e.get("toolCallId") == start["toolCallId"]]
        if len(ends) != 1 or ends[0].get("isError"):
            raise ValueError("Missing or failed send result")
        message = ends[0]["result"]["details"]["message"]
        receipts = message.get("receipts", [])
        if message.get("from") != sender or message.get("to") != recipient or len(receipts) != 1:
            raise ValueError("Wrong delivery receipt")
        receipt = receipts[0]
        if receipt.get("to") != recipient or receipt.get("outcome") not in {"woken", "injected", "revived"}:
            raise ValueError("Unsuccessful delivery outcome")
        incoming = [e for e in sessions[recipient][1] if e.get("type") == "custom_message"
                    and e.get("customType") == "irc:incoming" and e.get("details", {}).get("from") == sender
                    and e.get("details", {}).get("message") == payload]
        if len(incoming) != 1:
            raise ValueError("Missing or duplicate persisted recipient message")
        hops.append({"from": sender, "to": recipient, "payload": payload,
                     "tool_call_id": start["toolCallId"], "delivery": receipt["outcome"],
                     "recipient_message_id": incoming[0]["details"]["id"],
                     "recipient_timestamp": incoming[0]["timestamp"]})
    before_receipt = []
    for entry in sessions["SpecialistTwo"][1]:
        if entry.get("type") == "custom_message" and entry.get("customType") == "irc:incoming":
            break
        before_receipt.append(entry)
    if trace in json.dumps(before_receipt):
        raise ValueError("SpecialistTwo knew the trace before receiving it")
    final_seen = False
    received = False
    for entry in sessions["Main"][1]:
        if entry.get("customType") == "irc:incoming" and entry.get("details", {}).get("message") == expected[-1][2]:
            received = True
        message = entry.get("message", {})
        if message.get("role") == "assistant":
            parts = message.get("content", [])
            if any(p.get("type") == "text" and p.get("text") == f"ROUNDTRIP_COMPLETE {trace}" for p in parts):
                final_seen = received
    if not final_seen or report.get("shutdown") != {"exit_code": 0, "forced": False}:
        raise ValueError("No final acknowledgement after receipt, or unclean shutdown")
    identities = {}
    for name, (header, entries) in sessions.items():
        models = sorted({f'{e["message"]["provider"]}/{e["message"]["model"]}' for e in entries
                         if e.get("type") == "message" and e.get("message", {}).get("role") == "assistant"})
        if models != [report["requested_model"]]:
            raise ValueError("Unexpected or missing serving model")
        identities[name] = {"session_id": header["id"], "models": models}
    return {"passed": True, "scope": "One live native OMP round trip; no crash/recovery claim",
            "trace": trace, "elapsed_seconds": report["elapsed_seconds"], "sessions": identities,
            "hops": hops, "specialist_two_trace_first_arrived_in_peer_message": True,
            "shutdown": report["shutdown"], "raw_sha256": {
                "events.jsonl": hashlib.sha256((root / "events.jsonl").read_bytes()).hexdigest(),
                **{name: hashlib.sha256(path.read_bytes()).hexdigest() for name, path in files.items()}}}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    result = verify(args.input.resolve())
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"passed": True, "hops": len(result["hops"]), "sessions": len(result["sessions"]), "output": str(args.output)}))
