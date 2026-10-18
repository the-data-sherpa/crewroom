# Give specialists contained tools; defer the native harness inside a container

Design review finding R2 identified a limitation. Specialists could list or read whole files, replace whole files and run configured checks, but they could not search, make partial edits or run ad-hoc commands. On a 4,063-file repository this raises token use, rework and whole-file rewrite errors. The review asked for search and bounded partial edits under the existing writer/hash rules. It also asked for an ADR evaluating a contained full-harness mode: OMP's native tools running inside the per-attempt container from [ADR 0006](0006-contain-repository-commands.md).

## Decision

Specialists get three Crewroom-mediated tools. Their native OMP tools stay disabled.

- **`repository_search`** runs `git grep` over the owned worktree's tracked and non-ignored files, or, for an assigned reviewer, over its exact candidate revision. It is literal unless `regex` is set. Paths are plain repository paths, with no pathspec magic and no parent segments. It returns at most 100 lines of 300 characters.
- **`edit_file`** replaces exact text spans in one file read at `expectedHash`. Each span must match exactly once. The computed content enters the same writer-owned, hash-checked, durable edit ledger as `write_file`, and the ledger records the request hash, so a replay returns the same edit. A rejected patch never reaches the ledger.
- **`run_shell`** runs an ad-hoc `/bin/sh -c` command in its own ADR 0006 container, in the repository's admitted check image. The container has no network, drops all capabilities, has a read-only root filesystem and a writable `/tmp`, and mounts the worktree read-only. Its output goes to the agent only; it is never check evidence. Source changes still go through the writer's edits. Pause, stop and replacement use the existing container ownership path.

The contained full harness is **not adopted** for the MVP.

## Why the full harness is deferred

- **No Linux runner is available.** The installed OMP is a macOS arm64 executable. The local `@oh-my-pi` package ships only `pi-natives-darwin-arm64`. Running OMP inside the Linux check container needs a Linux OMP build and matching natives, and installing them is a new runtime dependency. The contract test for it cannot run until they are admitted.
- **The provider gateway needs network.** ADR 0006 containers have networking disabled. The agent inside the container would need a network restricted to the host's provider gateway: a custom network plus egress policy, not the tested `NetworkMode: none`.
- **Writes would bypass the edit ledger.** Native write tools would change the worktree directly, outside the writer's hash-checked edits. One-writer and interrupted-edit reconciliation would then depend on container ownership alone, and candidate commits would lose per-edit provenance. The mounted worktree would have to be read-only for every assignment except the writer's, and the writer's changes would need reconciling at each stop.
- **Process identity changes.** Host and OMP identity checks currently use leases on host processes. Inside a container they would need the container's durable identity as the owner.

The adopted tools remove the concrete limitations the review measured, without these changes. Revisit the full harness when a Linux OMP build is admitted as a pinned runtime dependency. Adopting it then requires a real-runner contract test covering pause and replacement with a live child command, a writer reconciling direct edits, and gateway-only egress.

## Evidence

- `tests/coding.test.ts` (R2): worktree search, including untracked and excluding ignored files; regex and path scoping; rejected escapes and pathspec magic; pinned-revision search; unique-span patches; stale hash, ambiguous span, non-writer and replay handling.
- `tests/container.integration.ts` (R2): the read-only mount allows reads and `/tmp` writes but rejects worktree writes.
- Real-runner contract: `python3 scripts/test-application-runtime.py --coding simple --coding-fault shell`. It uses real OMP, production hosts, Rust Agent Mail and Docker. A specialist runs `run_shell` with a detached child. The operator pauses during the live command, the container stops, and neither process writes further output. Ownership is confirmed stopped, and resume creates replacements without charging an automatic cycle. The replacement then uses `repository_search` and `edit_file`, and independent check and review certify the candidate. See the [progress record](../testing/mvp-progress.md#r2--specialist-tools-2026-09-29).
