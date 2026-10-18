# Design review — 2026-09-28

Scope: best-practice and design review of the Crewroom implementation at `a4ed59d` against the approved [product direction](product.md) and [architecture](architecture.md), compared with three software-factory projects: [Paperclip](https://github.com/paperclipai/paperclip), [firstmate](https://github.com/kunchenguid/firstmate) and the [Agentic Coding Flywheel](https://agent-flywheel.com/complete-guide). The operator asked to record these challenges and then work through every finding. Remediation items R1–R9 below are tracked in the [MVP plan](mvp-plan.md#design-review-remediation) and precede the continuation of M5.3.

## Summary

The product and architecture contracts are sound and stricter than all three comparators: intent is persisted before external effects, evidence is revision-bound, Jira status/workflow state/agent activity stay separate, and one writer owns each worktree. The weaknesses are in runtime construction. One of them directly causes the M5.3 blocker; another limits specialist output quality on real repositories.

## Comparator mapping

| Mechanism | Source | Crewroom at review | Decision |
| --- | --- | --- | --- |
| Atomic task checkout, no double work | Paperclip | `claimAttempt`, `claimRepositoryWriter` | Already covered |
| Revisioned configuration | Paperclip | Role/model snapshots per assignment | Already covered |
| Per-agent cost budgets | Paperclip | Active time only | Adopt actual usage accounting (R5) |
| Goal ancestry on every task | Paperclip | Free-text assignment input | Adopt structured briefs (R6) |
| Heartbeats, org chart, 24/7 autonomy | Paperclip | Operator-queued work only | Rejected: conflicts with single-operator, operator-queued scope |
| Single liaison agent | firstmate | Coordinator | Already covered |
| Event-driven supervision that wakes only when needed | firstmate | 100 ms serial polling pump | Adopt the principle (R3) |
| Ship vs. scout task shapes | firstmate | Coding vs. research outcomes (M6.1) | Already covered |
| Per-project delivery modes, `+yolo` merge | firstmate | One strict readiness policy | Not adopted for MVP; human merge remains |
| Terminal keystroke messaging | firstmate | Rejected by ADR 0004 | Remains rejected |
| Plan → dependency-graphed work units | Flywheel | Brief plus criteria | Adopt in structured briefs (R6) |
| Full-harness agents in a worktree | Flywheel, firstmate | Whole-file read/write and admitted checks | Adopt with container containment (R7) |
| Fungible swarm in one checkout with file reservations | Flywheel | One writer per worktree | Rejected: ADR 0002 is safer |
| Cross-run learning memory | Flywheel, Paperclip | None | Adopt as operator-approved proposals (R8) |

## Challenges

### R1 (P0) — Observation load consumes the recovery budget

Evidence at `a4ed59d`:

- Supervisor, HTTP API and repository assessment share one Node event loop.
- `RuntimeSupervisor.request` uses a 3-second abort timeout. In `pump()`, any host request failure sets `unavailable`, adds the assignment to `recoveries`, and `recover()` consumes an automatic cycle. No process inspection distinguishes a slow supervisor from a failed agent.
- `/api/workspace` rebuilds the full workspace snapshot for every browser poll (1 s per tab). `/api/runs/:id/runtime` returns the complete event history (about 12 MB measured). Repository status runs synchronous Git and file hashing (about 0.9 s per call measured).
- Inference: `AbortSignal.timeout` measures wall-clock time, so a host that answered promptly can be recorded as failed after the loop was blocked.

Required outcome: ordinary observation, including slow and multiple observers, leaves a running run executing with unchanged recovery counters. A host timeout with a live, identity-matching process is a degraded observation, not a failure, and never charges a cycle. Observation is incremental (cursor-based events, snapshots without full history); repository status is cached by revision identity and computed asynchronously.

### R2 (P1) — Agents have too narrow a toolset

Specialists receive `repository_read` (list or read an entire file), `write_file` (replace an entire file) and admitted checks. They cannot search, apply partial edits or run ad-hoc commands. On a 4,063-file repository this raises token use, rework cycles and whole-file rewrite errors. Comparator agents use their full harness inside a worktree.

Required outcome: specialists can search the owned worktree and apply bounded partial edits under the existing writer/hash rules, and a contained full-harness mode is evaluated under a new ADR: OMP native tools running inside the per-attempt container from ADR 0006, with only the owned worktree mounted and network egress limited to the provider gateway. The one-writer rule, candidate commits, revision-bound evidence and descendant-ownership contract remain unchanged. Adoption requires a real-runner contract test covering pause and replacement with a live child command.

### R3 (P1) — Supervision is one serial polling loop

`pump()` runs every 100 ms and serially fetches events, reconciles mail, dispatches peer messages, steering and delivery notices, and applies transitions. One slow host delays all others. `stopRun` and `close` busy-wait on `while (this.pumping)`. tmux calls use `execFileSync` with a 15-second timeout.

Required outcome: each host is observed independently so one slow host cannot delay another; transitions for a run are serialized explicitly; no busy-wait loops or synchronous child-process calls remain on the supervision path.

### R4 (P1) — `Workbench` is a monolith over JSON blobs

`server/workbench.ts` has about 70 methods spanning tickets, runs, attempts, messages, steering, publication, Jira, coding and events. Every table stores a `data` JSON column, so state cannot be indexed or queried. Lifecycle guards (for example `status !== 'running' || desiredState !== 'running'`) are repeated across methods. `agent_events` is unbounded.

Required outcome: run lifecycle transitions live in one pure, table-tested module; queried fields (run/attempt state, ownership, event sequence) are indexed columns with a migration that preserves existing databases, including the retained pilot; event storage has a retention/compaction policy that keeps receipts at full fidelity.

### R5 (P2) — Usage and cost are not recorded

The provider gateway sees every model request but does not record usage. Required outcome: per-attempt actual token usage is recorded from provider responses, labeled actual or unavailable (never inferred as zero), and shown in the workbench.

### R6 (P2) — Assignment briefs are unstructured

Assignment `input` is free text up to 40 KB, while review coverage already indexes criteria. Required outcome: assignments carry a structured brief — criteria, per-criterion obligations/checks, and dependencies on other assignments — snapshotted at admission and presented to the specialist and reviewer.

### R7 (P2) — Hard-coded single run and duplicated authorization

`start()` refuses when any host exists or any run holds unstopped attempts; the constructor requires exactly four host ports. M6.2 needs the next independent ticket to start once the preceding PR is ready for review. Host authorization is duplicated in six operation methods.

Required outcome: ownership checks are scoped per run while the first-release limit (one executing coding ticket) remains enforced as policy; host ports are allocated rather than fixed; one `authorizeHost` function replaces the duplicated checks.

### R8 (P2) — No learning loop

Recorded failures (for example the two check-image corrections) do not inform later runs. Required outcome: at a run's end the coordinator can propose role or project-instruction updates from recorded evidence; the operator approves or rejects each; approved changes are versioned like role snapshots and never alter historical assignments or validation policy.

### R9 (P3) — Documentation overhead

HANDOFF.md is 30 KB and mostly history; current status is restated in four documents; 21 research reports and 30 evidence files sit beside current material. Required outcome: the status snapshot is the single current-state source, HANDOFF.md is a short pointer plus resume prompt, and superseded research moves under `docs/archive/` with links preserved.

## Constraints on remediation

- Preserve the retained pilot run, its candidate, consumed time, recovery counters and check policy. Schema migrations are tested against a copy of the pilot database; the pilot data directory is never used by tests.
- Keep separate observable OMP processes, tmux visibility and external Rust Mail (ADR 0004, ADR 0005).
- Each item passes `npm test` and `npm run build`; items that change user flows also pass `npm run test:e2e`. Process-boundary changes add a focused real-process check.
- Record executed evidence in [the progress record](testing/mvp-progress.md).
