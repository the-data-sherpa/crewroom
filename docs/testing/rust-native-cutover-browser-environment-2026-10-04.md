# Retained browser environment correction — 2026-10-04

The disposable `46a8d14d873c15a6f6797f4d0e0627e2236b7749` clone passed all 31 retained browser journeys after its missing Mail executable was provisioned and its shared dependency symlink was replaced with a private `npm ci` installation. The source inputs remained unchanged. The clone has the proposed native startup aliases; this result does not apply the repository cutover or accept the packaged native GUI.

The original run remains recorded as 29/31. Both failed fixtures stopped before any provider request: `sandbox-exec` could not execute the absent clone-local `mcp-agent-mail`. Only the previously validated Mail 0.3.36 executable was copied, with its exact SHA-256 checked. Only the clone's verified `node_modules` symlink was unlinked; the main checkout's dependencies were untouched. The fresh build passed, then the full browser suite passed in 104.22 seconds. Its refinement and research process fixtures recorded 5 and 15 synthetic provider requests respectively, with no unexpected surviving processes.

The original 332/332 domain result and build result remain separately bound to their logs. No domain rerun was needed for an environment-only correction. The immutable original report and failed browser log, fresh dependency/build/browser logs, 175-file input manifest, process cleanup and Mail identity are linked by hash in the [machine-readable receipt](rust-native-cutover-browser-environment-2026-10-04.json).

Four `ghostty-web` `RangeError: offset is out of bounds` messages still appear in the passing feedback journey. The earlier accepted `n61c2b` browser log has the same four messages and matches its recorded hash. Fresh private dependencies did not remove them. This is a preexisting unresolved retained-browser diagnostic; it is not a claimed fix or evidence of a native migration regression.

The prepared README draft now lists OMP, tmux, running Docker and local fixture-image prerequisites, plus `python3 scripts/prepare-rust-mail-test.py` before the full browser suite. That helper verifies pinned Mail 0.3.36 and writes ignored fixture state. The draft remains outside main documentation until root completes cutover.

Progress: this environment blocker is closed, and all browser workloads exited. The final native GUI/resource and operating-system checks remain separate gates. The bounded repair stayed on track: it corrected the demonstrated missing prerequisite, preserved the failed attempt, and did not expand into unrelated legacy terminal changes.
