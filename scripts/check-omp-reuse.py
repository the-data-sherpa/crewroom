#!/usr/bin/env python3
"""Replay selected retired OMP contracts in a disposable copy, never in-place."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


FILES = [
    "vendor/queue_ledger.py",
    "tests/test_queue_ledger.py",
    *[f"live/src/{name}.py" for name in (
        "omp_team", "local_smoke", "github_adapter", "bitbucket_adapter", "onboarding"
    )],
    "live/tests/test_omp_team.py",
    "live/tests/test_local_smoke.py",
]
LEDGER_CASES = [
    "test_inherited_lock_survives_controller_death",
    "test_restart_uncertain_and_stale_completion",
    "test_failed_check_blocks_publication_and_repair_invalidates_gates",
    "test_source_drift_base_drift_and_evidence_corruption_block",
    "test_review_provenance_and_verification_receipt",
    "test_uncertain_publication_duplicate_intent_and_reconciliation",
    "test_priority_dependencies_require_merge_not_ready",
]
QUEUE_CASES = [
    "test_failed_local_check_blocks_push_and_one_repair",
    "test_lost_create_is_uncertain_and_never_blindly_repeated",
    "test_lost_create_with_zero_readback_stays_uncertain",
    "test_native_unknown_stops_batch_then_reopens_before_independent_work",
    "test_add_and_status_while_native_child_holds_execution_lock",
    "test_completed_impact_claim_resumes_without_duplicate_model_call",
]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path, help="Archived terraform-pilot-source/pilot directory")
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    source = args.source.resolve(strict=True)
    report = {
        "source": str(source),
        "scope": "Real disposable Git/SQLite/subprocesses; synthetic OMP and provider replies",
        "python": sys.version.split()[0],
        "files": {},
        "suites": [],
    }
    with tempfile.TemporaryDirectory(prefix="crewroom-omp-reuse-") as tmp:
        root = Path(tmp)
        for name in FILES:
            content = (source / name).read_bytes()
            destination = root / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(content)
            report["files"][name] = hashlib.sha256(content).hexdigest()
        # Tests inject the fake OMP and provider commands. Ignore the operator's
        # Git signing/hooks configuration while using real temporary Git repos.
        env = {key: os.environ[key] for key in ("PATH", "HOME", "TMPDIR", "LANG") if key in os.environ}
        env.update(GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM="1", PYTHONDONTWRITEBYTECODE="1")
        suites = [
            ("tests", "test_queue_ledger.QueueLedgerTests", LEDGER_CASES),
            ("live/tests", "test_omp_team.QueueTests", QUEUE_CASES),
        ]
        for directory, prefix, cases in suites:
            command = [sys.executable, "-m", "unittest", "-v", *[f"{prefix}.{name}" for name in cases]]
            try:
                result = subprocess.run(command, cwd=root / directory, env=env,
                                        capture_output=True, text=True, timeout=90)
                item = {"cases": cases, "exit_code": result.returncode,
                        "output": result.stdout + result.stderr}
            except subprocess.TimeoutExpired:
                item = {"cases": cases, "exit_code": None, "error": "90-second suite timeout"}
            report["suites"].append(item)
            print(f"{prefix}: {len(cases)} cases, exit {item['exit_code']}", flush=True)
    report["passed"] = all(suite["exit_code"] == 0 for suite in report["suites"])
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2) + "\n")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
