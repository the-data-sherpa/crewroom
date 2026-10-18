# MH2 implementation readiness — FOR-7

Evaluated 2026-10-12 against source `686c26b852a614e2f5a68f1c899f3a9bd91a6137`. Linear: [FOR-7](https://linear.app/forgeinnovations/issue/FOR-7/mh2-runtime-interface-and-codex-adapter).

**The bounded compatibility spike is now complete; proceed with the runtime refactor and production integration gates. Codex execution is not enabled.** The [follow-up results](codex-mh2-compatibility-spike-2026-10-12.md) supersede this review's original gateway-first recommendation: the operator clarified that harnesses supply model access through their existing logins. Two live native-auth turns and 23 synthetic cases per tested profile passed. Production receipt reconciliation, credential refresh, truthful budget capabilities and full adapter acceptance remain outstanding. The source mapping below remains the implementation handoff.

## Scope and decisions

MH2 adds a Codex adapter alongside OMP under the existing Rust workflow authority. Each coordinator/specialist retains a separate host and runner process, external Mail, owned tools, durable observations and independent review. MH1 discovery is complete. Claude Code is MH3; the unified picker is MH4; intake/pairs remain MH5/MH6.

Accepted constraints come from [product](../product.md), [architecture](../architecture.md), [multi-harness plan](../multi-harness-plan.md), and [contained tools](../adr/0008-contained-specialist-tools.md). Reuse the existing ownership, evidence and recovery mechanisms identified in the [OMP reuse assessment](omp-team-reuse-assessment.md); keep the retired installation inactive.

| Decision | Proposed implementation position | Status |
| --- | --- | --- |
| Workflow/process topology | Keep the supervisor, one host plus one runner per assignment, external Mail and current coding/writer limits. | Accepted |
| First Codex model-access route | Existing native Codex authentication, private assignment state and explicit model/effort. Retain the OMP gateway separately. | Operator clarified; bounded live check passed |
| Existing Codex/ChatGPT login | Reuse current configured access. Keep refresh/account ownership server-side; expose only verified enforcement capabilities. | Live access proved; production refresh/accounting integration remains |
| Tools | Flat experimental dynamic functions invoke existing Crewroom host tools through a pinned restricted profile. | Bounded profile/roundtrip proved; production authority checks remain |
| Session storage | Persistent private Codex state per assignment; fresh ownership root per process attempt. Resume by saved thread ID after ownership reconciliation. | Durable completed thread proved; ambiguous effects need Crewroom receipt reconciliation |
| Legacy records | Absent adapter tag means legacy OMP only. Preserve old provider/model strings, session paths and ownership roots. Reject unknown explicit tags. | Required compatibility behavior; additive representation proposed |
| MH2 configuration surface | Explicit server-side route for disposable/new assignments; catalog UI and equivalence mapping remain MH4. | Proposed slice boundary |

The initial gateway recommendation was superseded by the operator's clarification. Its synthetic tests remain diagnostic evidence. Native authentication is now the MH2 implementation route; do not require the operator to provision a separate Responses API key. Accepted queue, review and delegation decisions remain unchanged.

## What the code already provides

The seam must include lifecycle and identity, not just JSON framing.

| Existing module | Reuse / required change |
| --- | --- |
| [host.rs](../../crates/crewroom-runtime/src/host.rs) | Keep authenticated local control, journals, task draining and observation. Extract OMP config generation, session open, model setup, input dispatch, tool response encoding and interrupt calls. |
| [rpc.rs](../../crates/crewroom-runtime/src/rpc.rs), [rpc_frame.rs](../../crates/crewroom-runtime/src/rpc_frame.rs) | Preserve OMP v2 behavior in its adapter. Codex needs its own JSONL request/notification/server-request decoder; it does not use OMP chunk envelopes. Both need bounded framing and unresolved-ID handling. |
| [workflow/configuration.rs](../../crates/crewroom-runtime/src/supervisor/workflow/configuration.rs) | Split frozen route preparation from OMP-specific command flags, private files and provider configuration. No provider error may silently select another route. |
| [workflow/start.rs](../../crates/crewroom-runtime/src/supervisor/workflow/start.rs), [hosts/launch.rs](../../crates/crewroom-runtime/src/supervisor/hosts/launch.rs), [hosts/recovery.rs](../../crates/crewroom-runtime/src/supervisor/hosts/recovery.rs) | Today these assume `omp-process`, `ompPid` and OMP session directories. Add adapter-aware ownership/session identity consistently across launch, reattachment and stop. Keep legacy path lookup intact. |
| [attempts.rs](../../crates/crewroom-core/src/attempts.rs) | Strict identity validation currently requires `ompPid`. Add a versioned generic identity for new routes and a legacy reader; do not relabel Codex as OMP or rewrite historical records indiscriminately. |
| [host/tools.rs](../../crates/crewroom-runtime/src/host/tools.rs), [workflow/tools.rs](../../crates/crewroom-runtime/src/supervisor/workflow/tools.rs) | Retain catalog routing, host authorization, current-attempt checks, writer/hash rules and contained commands. Translate only at the adapter seam; recheck authority when queued calls execute. |
| [host_journal.rs](../../crates/crewroom-runtime/src/host_journal.rs), [workflow/delivery.rs](../../crates/crewroom-runtime/src/supervisor/workflow/delivery.rs) | Keep durable input disposition and fail-closed unknown dispatch. Add persistent input-to-thread/turn and call-to-original-operation mapping for Codex recovery. |
| [provider_gateway.rs](../../crates/crewroom-runtime/src/provider_gateway.rs), [provider_input.rs](../../crates/crewroom-runtime/src/provider_input.rs) | Preserve request/model/effort/history/budget checks. Codex's actual Responses bodies and compaction behavior must pass a captured compatibility test before changing these guards. |
| [observations.rs](../../crates/crewroom-core/src/observations.rs), [observation.rs](../../crates/crewroom-runtime/src/observation.rs), [pane.rs](../../crates/crewroom-runtime/src/pane.rs) | Keep coalescing and final-event durability. Normalize runner events for existing state consumers; raw Codex wire details stay in the adapter. Terminal text never establishes completion. |

## Proposed runner interface

Introduce a `runner` module with OMP and Codex adapters. Keep discovery as the existing separate MH1 operation until catalog work requires integration. The host learns a small interface:

- **Open:** consume a frozen route, assignment instructions, tool catalog and optional resume reference; return verified process/session/model identity or an explicit failure.
- **Submit:** accept a durable Crewroom input ID and either normal input or steering for a known active turn. Return acknowledged/unknown disposition; a write to stdin alone is not receipt.
- **Observe:** emit normalized message/tool/usage/activity and terminal events, preserving assignment, attempt, input, turn and call identity.
- **Resolve tool:** return an outcome to the original server request while retaining the logical call identity separately.
- **Interrupt and close:** request interruption, stop admission, settle host work, then use existing process/container ownership checks. An interrupt acknowledgment cannot certify stopped ownership.

An enum-backed implementation is sufficient for two adapters; a general plugin registry is unnecessary. Protocol settings and session controls belong inside adapters; workflow admission, tool effects, candidate evidence and retry policy remain with their existing owners.

Freeze new route metadata (adapter kind, configured executable/version expectation, provider/model selector, effort, capability/policy profile version) when admitting an assignment. Record observed executable/hash and effective identity on launch. Keep credentials out of those public snapshots. Pin/reject unsupported versions until their contracts are retested.

Use current normalized event names where they carry the right meaning, with versioned identity additions. Map Codex `turn/completed` statuses explicitly: completed → input completed, interrupted → aborted, failed → error. A retrying error, final text, process exit or tool item completion is not successful assignment completion. Steering inputs can share a turn, so persist their individual receipt links. Fail on missing/conflicting identity rather than guessing idle or success.

## First implementation gate: compatibility evidence and remaining integration

The [completed spike](codex-mh2-compatibility-spike-2026-10-12.md) covers the bounded protocol checks below. It used private namespaces/state and an explicitly selected binary. The live check reused native authentication; it did not traverse the gateway. The remaining work is production enforcement and recovery integration, not another initial route selection.

1. **Captured synthetic exchange: passed within the pinned profile.** Flat dynamic functions, bounded history and explicit native-call denial worked. Feature flags alone did not suffice; retain the tested startup model catalog and configuration. Production wrong/stopped-caller and contained-tool checks remain required.
2. **Native authenticated exchange: passed.** The existing ChatGPT login completed an addressed harmless tool result and a second turn after process replacement. Wall time and tool count bounded the probe; reported usage is not a strict token/spend cap. Integrate the architecture's active-time and truthful accounting rules for this route.
3. **Thread persistence: passed after a real turn; uncertain effects remain a host responsibility.** A metadata-only start did not establish durable history. The subsequent crash matrix showed that an unanswered call becomes `aborted` even after an effect or host receipt exists. Reconcile the original operation before replacement work; the protocol alone does not provide exactly-once effects.
4. **Extract and integrate the OMP seam next.** Preserve its controls and add native route capability checks explicitly. Production refresh, receipt recovery, external Mail and stopped-ownership tests remain acceptance gates; no source adapter has been enabled by the spike.

Protocol fields, exact candidate configuration, caveats and official citations are in the [Codex protocol findings](codex-mh2-protocol-readiness-2026-10-12.md). The probe's feature switches are research inputs, not a proven production deny policy.

## Replay and migration rules

Codex has at least five different identities: Crewroom input ID, RPC request ID, thread ID, turn ID and tool call ID. Persist their relationships before admitting side effects. Replies use the server request ID; business deduplication uses a stable logical operation tied to the original assignment/session/call and argument fingerprint.

The existing backend usually deduplicates by attempt plus tool-call ID. Replaying the same logical call under a replacement attempt can therefore create a new effect. Recovery needs a lookup of the original authorized operation and retained receipt, without granting the stopped attempt fresh authority. Reuse the existing per-tool reconciliation ledgers; add adapter mappings and any missing result persistence. If an effect or delivery is unknown, require reconciliation; do not blindly resubmit or claim exactly-once execution.

Test old untagged OMP assignments, stored sessions, live reattachment, paused resume and unknown/malformed new route records. New Codex session stores remain private and assignment-scoped; never adopt a thread from a different assignment. Existing OMP ownership roots and `ompPid` fields stay readable. New runner identity must reach terminal observation and recovery consumers together.

## Implementation order and exit evidence

| Step | Work | Exit evidence |
| --- | --- | --- |
| A | Bounded spike complete; use native harness access and pinned restricted profile. | Captured tool/request inventory, live native-auth roundtrip/restart and explicit crash behavior recorded. Production refresh/reconciliation gates remain. |
| B | Extract OMP into the runner module with additive route/identity records. | Existing host/RPC/supervisor/ownership tests and legacy migration/reattachment cases pass without behavior loss. |
| C | Add Codex JSONL, dynamic tools, input/turn mapping and event normalization. | Correlated success/error calls; malformed/oversized frames; wrong assignment/thread/turn; duplicate calls; unexpected native requests all handled. |
| D | Integrate stop, restart and Mail delivery. | Busy streaming/tool interruption; descendants stopped; daemon reattaches same live host; dead host resumes usable thread or records replacement; no duplicate tools or writers. |
| E | Exercise new assignments through the Rust service; retain OMP support. | Real disposable acceptance matrix below, required regression checks and revision-bound receipts. No unified picker claim. |

## MH2 acceptance matrix

| Case | Required observation |
| --- | --- |
| Model policy | Frozen selected model/effort matches request and returned identity; fallback disabled; unavailable route fails visibly. |
| External message/tools | Two distinct host/runner pairs; addressed send/reply goes through Crewroom and external Mail; results reach only the calling session. |
| Tool authority | Non-writer mutations, wrong/stopped attempts, unknown tools, native bypass and late calls after Pause are denied. |
| Busy steering | Correct active-turn targeting; saved, received and applied steering remain distinct through restart. |
| Stop | Streaming and contained tool work stop; an interrupt response alone leaves the run pausing until all ownership settles. |
| Recovery | Daemon crash with living hosts reattaches; host/runner crash resumes durable history when usable; empty/corrupt/wrong-session state is explicit. |
| Replay | Fault injection around intent, effect, receipt and reply produces one logical effect or a visible unresolved state; no blind retry. |
| Observation/limits | Deltas are bounded; final events persist; provider retry/failure and usage are accurate; active-time accounting and budget termination remain effective. |
| Legacy OMP | Historical assignments/session roots keep original route; current OMP contract and stopped-ownership tests still pass. |

For implementation: run targeted Rust host/runner/core/service tests and guarded real-runner fixtures, then `npm test`, `npm run build`, and `npm run test:e2e` for affected domain/build/user flows, plus native interaction tests for any changed views. Bind evidence to the exact implementation revision, executable identities and platform. Separate fake wire peers, real runners with synthetic providers, and live inference.

## Initial evaluation baseline (before the follow-up spike)

- Codex: **0.162.0**, installed version-specific schemas generated; isolated initialize/start/resume metadata probe executed with **zero inference turns**.
- OMP: **17.3.5** on this machine. The existing Rust transport targets **18.4.12 protocol v2**. Version mismatch requires a pinned compatible fixture and real contract verification; this review does not prove installed 17.3.5 incompatible or upgrade it.
- Node: shell selects **22.23.3**; `package.json` requires **>=24**. Select a compliant toolchain for application validation without changing global defaults.
- Docker: server reports **29.7.2**. That proves daemon availability only, not image availability or contained-command acceptance.
- Existing real OMP fixtures in `scripts/omp_offline_fixture.py` and `scripts/rust/verify-omp-*.py` embed macOS paths, native-addon names and `sandbox-exec`. Port the guarded fixtures for Linux, or run them on the evidenced macOS environment. Removing their guard is not a valid portability fix.
- Focused current-source baseline: **23/23** Rust tests (11 host protocol, 3 model jobs, 2 actual process-ownership, 7 simulated RPC) and **9/9** discovery fixture/process tests pass.
- Full domain/build/browser/native release suites were not rerun for this documentation evaluation. Retain the consolidation record's **29/31** browser limitation; do not convert it to an MH2 pass.

See [readiness receipt](../testing/mh2-readiness-2026-10-12.json), [status](../status.md), and [progress](../testing/mvp-progress.md). FOR-7 remains unfinished until implementation and its acceptance gates pass; MH3 is still dependent on it.

