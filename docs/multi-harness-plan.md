# Model-based specialist assignments across harnesses

Started 2026-10-12 at the operator's request, based on `main@dc85a54`. MH1 was committed as `d293130` on `feat/multi-harness`. Development is now consolidated into `/home/crewroom/Projects/crewroom` on `main`; the original local research and approved S9 plan are retained here.

## Accepted direction and current scope

Specialists have roles, instructions and a selected model. Crewroom should discover model offerings from OMP, Codex and Claude Code, then resolve a model selection to a compatible harness. The normal assignment UI should focus on models. The actual harness, executable version, model selector and observed resolved model remain visible in assignment details and durable evidence.

The operator also confirmed the intended factory flow: queue a job for a repository; use an existing implementation plan when current; otherwise scope it through a grilling conversation; select an appropriate team; run a Codex/Opus programming pair with independent review; include documentation and UI specialists when the task needs them. These are the next workflow changes, not claims that they are already delivered.

The operator clarified on 2026-10-12 that the harnesses supply model access through their existing authentication. Crewroom should reuse that configured access; a separate API-provider setup is not a prerequisite for native Codex/Claude routes. Keep authentication and refresh server-side, with private assignment state and explicit enforcement capabilities. The existing OMP gateway remains supported.

The repository supports OMP, native Codex (MH2) and native Claude Code (MH3) through the same workflow authority. The [Claude contract](testing/mh3-runtime-acceptance.md) records explicit configuration and recovery/capability limits. These source changes do not change the active installation, role files, model settings or task queue. MH1 below remains discovery-only evidence.

MH2–MH6 are implemented and validated within their recorded limits; their revision-bound evidence is in [status](status.md). MH6 / FOR-11 is supported by the [pair contract](testing/mh6-pair-contract.md) and [live acceptance receipt](testing/mh6-pair-2026-10-13.json), including retained failures and recovery limitations. The earlier approved [S9 verification and evaluated-learning work](workbench-redesign.md#s9--verification-and-evaluated-learning) remains planned follow-up work after this current track. The [specialist workflow assessment](research/crewroom-specialist-workflow-assessment-2026-10-12.md) records the source inspection behind the factory direction; this plan records the subsequently confirmed runtime choice.

## Next MVP boundary

Confirmed by the operator on 2026-10-12: the next MVP is the multi-harness factory through MH6, including **OMP, Codex and Claude Code**, model-based selection, plan-aware intake and programming pairs. OMP remains a supported execution adapter: retain existing assignments/sessions, pass its runtime contracts through the refactor, and exercise it in the unified model-selection flow. S9 remains approved follow-up after this MVP. The original M0–M7 release is already accepted; these are the next milestone's requirements.

Completion requires the MH2–MH6 evidence below, including real adapter contracts, successful model selection through all three verified adapters, planned/unplanned intake, and real backend/UI pair workflows with independent review and interruption recovery. Platform support claims retain their existing limits until separately evidenced.

## Accepted delegation decisions

Confirmed by the operator on 2026-10-12 after the [Orchestrator assessment](research/orchestrator-delegation-assessment-2026-10-12.md); recorded in [FOR-16](https://linear.app/forgeinnovations/issue/FOR-16/record-accepted-delegation-scope-for-mh5-and-mh6). These decisions constrain MH5/MH6 implementation; current execution is recorded in status.

- **Coordinate within each run.** Use the existing run coordinator to delegate named assignments and collect progress, outputs and blockers. Crewroom's service retains workflow and queue authority.
- **MH5 / FOR-10:** the accepted implementation plan records named work packages, dependencies, expected outputs and acceptance criteria. Reuse a current plan; resolve missing decisions through the planned intake flow. Proposed assignments, queued work and running agents remain distinct.
- **MH6 / FOR-11:** translate the work packages into frozen role/model/route briefs for the writer, navigator and staged specialists. Show assignment ownership, meaningful reports, artifacts and blockers through the existing agent grid and coordinator conversation.
- **Retain one executing coding ticket at a time**, with at most three compatible specialists plus the coordinator and one writer per worktree. Existing queue release, dependency and writer-transfer rules still apply.
- **Require explicit operator Resume for PR-feedback work.** Continue observing feedback and invalidating stale readiness; observations do not automatically admit execution. Rework retains the owned branch/PR and needs fresh revision-bound checks and independent review. Human merge remains the final gate.
- **Preserve OMP, Codex and Claude Code**, separate observable agent processes, external messages, interruption recovery and independent final review.

Validate these details within the existing planned/unplanned intake, backend/UI pair and interruption scenarios, including the retained queue and PR-feedback controls. A persistent project-wide lead, multiple simultaneous coding tickets and automatic PR-feedback admission are possible later expansions, outside this MVP. Revisit them if experience warrants a change, with an explicit decision and validation of the affected contracts.

## Interface and model identity

Keep workflow authority in Crewroom's Rust service. Introduce a runner module with three adapters behind a small interface: discover model offerings; start/resume a session with a frozen assignment; send input; interrupt/stop; observe normalized events and identity. Harness-specific wire formats, model aliases and session controls belong inside the adapters. Reuse the current process supervision and external message transport.

Preserve these distinct facts:

- **Role:** reusable responsibility and procedures, such as implementer or UX specialist.
- **Model selection:** the operator's intended model, plus supported effort settings or inherited job defaults.
- **Model offering:** a harness-specific selector and advertised capabilities. The same display name does not establish equivalence across harnesses or providers.
- **Resolved route:** the concrete harness/model configuration selected at admission and retained with the assignment. A provider error must not silently choose another harness or model.

The [MH4 contract](testing/mh4-model-contract.md) defines distinct offering keys and human-readable model names. It preserves Claude aliases separately from their observed model, and distinguishes OMP providers. Do not collapse `default` into a permanent model identity. Explicit equivalence mappings and ordered route preferences are required before grouping offerings; material capability/access distinctions remain visible. No mappings are installed by default. The user chooses the model, while configuration resolves explicitly equivalent routes without forcing a harness choice for every job.

Catalog visibility, configured credentials, successful inference and validated execution support are separate states. Unknown effort/input/tool capabilities remain unknown. Existing OMP assignments and stored sessions retain their original route; migrations must not reinterpret their provider/model strings as new harness IDs. Initially only new assignments may use new adapters after contract validation.

## Ordered work and acceptance

| Item | Work | Completion evidence |
| --- | --- | --- |
| **MH1 — Discovery contract probe** | Query each installed harness without an inference prompt; normalize model metadata; preserve source identity and unknown capabilities; report unavailable harnesses independently. | **Completed within probe scope.** Nine fixture/process tests plus a real discovery run against all three installed CLIs. [Receipt](testing/harness-discovery-2026-10-12.json). |
| **MH2 — Runtime interface and Codex adapter** | Extract the OMP-specific protocol seam without replacing Crewroom supervision. Implement Codex app-server initialization, assignment/session identity, tools, event streaming and stop/resume. | Real disposable runner cases: correct model, addressed tool roundtrip, independent processes, busy interruption, confirmed stopped ownership, session recovery, no duplicate tool delivery after restart. Existing OMP contracts still pass. |
| **MH3 — Claude Code adapter** | Implement a separately supervised Claude Code process through its supported streaming/control integration; use Crewroom's tools and external mail. | Same cases as MH2. Validate permission/tool gating on every relevant call, distinguish interruption from completion, and retain model alias versus observed identity. Initialization alone does not pass this item. |
| **MH4 — Native model catalog and assignment** | Add explicit discovery refresh, stale/error states, model/job selection, route resolution and frozen assignment details to the Rust service and native UI. | A specialist can select and run an eligible model through each verified adapter. Duplicate labels, unavailable routes, unsupported efforts, missing tools, restart, legacy assignment history and credential-free UI are exercised. |
| **MH5 — Intake and accepted implementation plans** | Separate queued ideas from execution-ready jobs; automatically enter grilling when a current plan is absent; permit controlled repository inspection during scoping. | A planned job skips redundant questions. An unplanned job asks for missing decisions, saves the accepted plan, and advances under existing queue authority. Changed requirements invalidate the affected plan; operator wait survives restart without consuming execution budget. |
| **MH6 — Pair and specialist choreography** | Define writer/navigator responsibilities, structured exchanges, optional UI/diagnosis specialists, documentation obligations, and independent final review. | A real backend job and UI job assemble different teams. A navigator meaningfully challenges the implementation. Documentation and all changes precede final evidence; a third-party reviewer is independent. Interrupted work resumes without duplicate writers. |

This sequence is a development plan, not permission to use unrelated live tickets or alter an active installation. Use disposable worktrees for contract tests. The current single coding slot and three-specialist limit remain in force until a separately tested policy change is justified.

MH2 preparation is captured in the [implementation readiness review](research/mh2-implementation-readiness-2026-10-12.md) and [installed Codex protocol findings](research/codex-mh2-protocol-readiness-2026-10-12.md). The subsequent [compatibility spike](research/codex-mh2-compatibility-spike-2026-10-12.md) passed 23 synthetic cases per tested model profile plus two live turns through the existing Codex login. It establishes protocol/profile feasibility and the need to reconcile host receipts after interrupted calls; the production adapter and its later acceptance are recorded in the [MH2 runtime contract](testing/mh2-runtime-acceptance.md) and [progress record](testing/mvp-progress.md). Native capability limits remain explicit. Retain the existing OMP and workflow contracts throughout the refactor.

## Pair and documentation design constraints

The smallest initial pair is one writing agent and one active navigator. Existing messaging can support that pattern. The navigator is a collaborator, not the independent final reviewer. Preserve a distinct final review assignment, including when a navigator never personally invoked a write tool.

Arbitrary writer alternation is not presently supported. After a coding candidate exists, the engine admits a new writer only under recognized rework/continuation conditions. Add an explicit implementation stage and ownership transfer if both pair members or a technical writer need to edit before candidate completion. Do not invent a failing review to unlock another writer. Freeze the candidate only after required implementation/documentation responsibilities have closed, then require checks and independent review against that exact revision.

Three active specialists can accommodate the pair and reviewer. UI and documentation roles can participate in different stages through existing retirement, provided the implementation-stage contract permits the required work. A UI specialist also needs appropriate visual tools and acceptance evidence; its role name is insufficient.

## MH1 operation and observed limits

Run from this checkout:

```sh
python3 scripts/discover_harness_models.py
python3 scripts/discover_harness_models.py --harness codex
python3 -m unittest discover -s scripts -p 'test_harness_discovery.py' -v
```

`--codex`, `--claude-code`, and `--omp` accept explicit executable paths. Output is normalized JSON on stdout; any selected unavailable/failed harness makes the command exit nonzero while preserving successful results from the others. The bounded commands never send a user prompt, create a Codex thread, or start an inference turn. Only model metadata is emitted, not raw initialization, account details, environment values, credentials or stderr.

| Installed CLI | Observed version | Advertised entries | Meaning |
| --- | --- | --- | --- |
| Codex | 0.162.0 | 7 | `initialize` then paginated `model/list`; client catalog, access unverified. |
| Claude Code | 2.1.292 | 5 | Isolated initialization menu without credentials; includes aliases, not five necessarily distinct underlying models. |
| OMP | 17.3.5 | 109 | Configured authentication/keyless-provider catalog; inference access unverified. Older than the repository's recorded macOS runtime. |

Claude discovery deliberately uses a temporary config, no hooks, no tools and no MCP servers. It does not claim to mirror account-specific restrictions. OMP listing can refresh caches or resolve authentication; it is non-inference, not guaranteed offline or filesystem-read-only. Codex uses its current local configuration from an empty temporary working directory. Counts are a dated observation, not pinned product options.

The process probe stops and reaps its own child/process group. It is not the production containment contract for arbitrary commands or detached descendants. Production adapters still require Crewroom's stronger ownership, containment, recovery and tool authorization tests.

MH1 changes only Python discovery tooling and documentation. No Rust/TypeScript domain or user-flow change was made, so validation is the focused Python suite and actual metadata exchanges; no new application suite or delivery pass is claimed.

## Primary sources

- [Codex app-server](https://learn.chatgpt.com/docs/app-server): initialization, model listing and the persistent session/turn interface. Locally verified against 0.162.0. Model availability and efforts come from the response rather than a hard-coded list.
- [Claude Code and OMP discovery findings](research/harness-discovery-2026-10-12.md): exact tested commands, source links, response shapes and limitations.
- [Current Crewroom runtime](../crates/crewroom-runtime/src/supervisor/workflow/configuration.rs), [model policy](../crates/crewroom-runtime/src/model_policy.rs), [writer claims](../crates/crewroom-runtime/src/repository/claims.rs), and [independent review](../crates/crewroom-runtime/src/evidence/reviews.rs).
