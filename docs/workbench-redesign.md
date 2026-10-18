# M8 — Operator workbench redesign

Status: direction approved by the operator on 2026-10-04 after reviewing the [interactive mock](design/workbench-redesign-mock.html) ("I really like this, let's run with it"). This plan sequences the native implementation. It does not change product requirements in [product.md](product.md); every invariant in `AGENTS.md` still applies.

**Scope addition, 2026-10-04:** the operator approved adding the new ideas from the [resource assessment](research/agent-workflow-resources-assessment-2026-10-04.md) as S9, after S8. S9 adds verification recipes, enforceable corrections and comparative instruction evaluation; it does not repeat S5–S8's model jobs, principles, role history or playbook catalogs. S9.1–S9.3 are now accepted within the bounded evidence below. The original mock covers S1–S8 only.

**Implementation status, 2026-10-14:** S1–S8 are accepted within their recorded macOS scope, and the [multi-harness track](multi-harness-plan.md) through MH6 is complete within its recorded limits. The operator requested the [FOR-12 / S9.1 implementation plan and testing constraints](testing/for-12-implementation-plan.md), then authorized implementation. S9.1 is implemented and validated at `a3a9848`; its [source-bound receipt](testing/for-12-feature-map-2026-10-13.json) records fresh-specialist acceptance of all three recipes and the corrected stale-selector rerun. FOR-13 / S9.2 is also implemented and validated within its [correction/mutation receipt](testing/for-13-corrections-2026-10-13.json); FOR-14 / S9.3 is implemented and validated within its [comparison receipt](testing/for-14-comparison-2026-10-14.json). The ordering and acceptance cases within S9 are unchanged.

## Destination

The operator supervises a run from one window. Selecting a ticket opens its session: the coordinator's live terminal and steering sit above at most three specialist tiles, with the brief, results and history in a drawer beside them. Pending decisions are collected in **Needs you**. Configuration sits behind a settings gear. Specialists are created by describing them, improved through reviewed proposals, and composed from a model *job*, attachable *principles* and, per ticket, a *playbook*.

The mock is the visual and interaction reference. It uses example data and scripted agents; it is not evidence for any behavior below.

The [harness engineering assessment](research/harness-engineering-and-crewroom.md) records the 2026-10-06 discussion of development, validation, and improvement loops. Its proposed evaluation work is separate from the slice acceptance cases below.

## Invariants carried into every slice

- Jira status, workflow state, agent activity and readiness are four separate facts in the sidebar and the session header.
- A proposed or queued specialist is drawn without a terminal and labelled *not started*. Only an admitted attempt with a process gets a terminal tile.
- Specialist terminals stay read-only. Steering goes to the coordinator and keeps its stored → received → applied states.
- Playbook steps are a plan. They never satisfy readiness; only revision-bound checks and independent review do.
- Role, principle and job changes affect new assignments only. Each assignment keeps the role revision, principle text, model and effort it was admitted with.
- Drafting, "Ask for a change" and "Teach this role" produce proposals. Nothing changes a role without the operator's explicit approval.
- Unavailable integrations (runner, Jira, mail) are shown as unavailable, and features that need them say so instead of failing silently.

## Slices

Slices are taken in order. Each is implemented, tested, accepted with a progress entry, and committed before the next starts. UI-only slices come first so the operator sees the new shell early; schema and runner-contract changes follow.

### S1 — Session shell and sidebar (UI only)

**Accepted 2026-10-04**; see the [progress record](testing/mvp-progress.md#m8-s1--session-shell-and-sidebar-2026-10-04).

**Outcome:** the main window matches the mock's frame: sessions sidebar (Running / Queue / Other) with the three fact chips per ticket, session header with the four-fact bar, Brief / Results / History moved into a toggleable drawer, a **Needs you** entry with a count, and settings behind a gear.

**Reuses:** `workbench.rs` panels, `navigation.rs` saved state, existing tab content.

**Acceptance cases:** facts stay separate for running, queued, paused-at-review and refining tickets; navigation and unsaved drafts survive switching sessions, opening settings and reopening the app; keyboard reachability and no horizontal overflow at 390 px.

### S2 — Embedded agent grid

**Accepted on macOS 2026-10-06**; see the [acceptance record](testing/mvp-progress.md#m8-s2s3--macos-acceptance-and-accessibility-repair-2026-10-06). Linux/Wayland terminal verification remains an explicit follow-up.

**Outcome:** the session embeds the coordinator pane (terminal or timeline, steering composer and delivery states) and up to three specialist slots. Slots show live attempts, proposed specialists (dashed, no terminal) and empty "Add a specialist" slots that open the existing insertion form as a grouped, searchable role picker. Required checks appear as a strip with their revisions. Any tile can be maximized.

**Reuses:** `wall.rs` observation transport and Ghostty panes, `collaboration/conversation.rs` steering, `collaboration/insertion.rs` proposal path, agent events for the timeline view.

**Open decision:** keep the separate agent-wall window as an optional "Pop out", or retire it. Recommendation: keep it as a pop-out for this slice because its close/reopen behavior is already tested; revisit after S2 is in use.

**Acceptance cases:** a proposed specialist has no terminal until admitted; the team cap of three is enforced in the picker and server-side; steering states advance only on recorded transitions and wait while paused; closing the window drops observers only; terminal rendering verified on the macOS package. On 2026-10-06 the operator approved continuing S4–S8 after macOS validation, with Linux/Wayland terminal verification retained as an explicit follow-up.

### S3 — Needs you inbox (read projection + existing actions)

**Accepted 2026-10-06**; see the [on-screen acceptance record](testing/mvp-progress.md#m8-s2s3--macos-acceptance-and-accessibility-repair-2026-10-06).

**Outcome:** one list of pending operator decisions with their actions: repair-limit grants, learning proposals, source changes needing requirement adoption, PR feedback ready for Resume, uncertain publication/Jira outcomes, and uncertain ownership. The sidebar shows the open count.

**Reuses:** existing endpoints for `grantRepairCycle`, `decideLearning`, `adoptRequirements`, `queueFeedbackResume` and reconciliation. Adds only a read projection.

**Acceptance cases:** each kind appears when its underlying state exists and disappears when resolved elsewhere; actions go through the same server checks as their current screens; no item is created from terminal output alone.

### S4 — Role catalog and editor sheet (existing schema)

**Accepted on macOS 2026-10-06** at `7516f60`; see the [progress record](testing/mvp-progress.md#m8-s4--role-catalog-and-prompt-preview-2026-10-06).

**Outcome:** roles appear as cards grouped by category, with the "Pick when" text, the resolved model and effort, the enabled state or a lock with its reason, usage counts and pending proposals. Editing happens in a side sheet with a generated ID for new roles, Duplicate, and a read-only **assembled-brief preview** for a chosen ticket.

**Reuses:** `settings.rs` role drafts and revision conflict handling, `roles.rs`, `briefs.rs` snapshotting for the preview.

**Acceptance cases:** existing ROL-02/ROL-03 conflict and snapshot behavior unchanged; core-role lock explained; the preview matches what admission would snapshot for the same inputs.

### S5 — Model jobs and budget (schema change)

**Accepted on macOS 2026-10-06** at `54f8c3c`; see the [progress record](testing/mvp-progress.md#m8-s5--model-jobs-and-budget-2026-10-06).

**Outcome:** roles choose a job (Code, Judgment, Review, Fast). Runtime configuration maps each job to a model and holds one budget level (Unlimited, Large, Medium, Small) that sets effort per job. A role's model and effort become optional overrides. Admission resolves and snapshots the concrete model and effort.

**Migration:** saved roles with an explicit model or effort keep them as overrides; default roles get a job by ID. Unknown or missing job falls back to Judgment with a visible warning.

**Acceptance cases:** resolution order (role override → job model; role effort → budget) is tested in the domain; a job or budget change does not alter admitted assignments; model availability is still checked before launch.

### S6 — Principles (schema change)

**Accepted on macOS 2026-10-06** at `c3280dd`; see the [progress record](testing/mvp-progress.md#m8-s6--principles-2026-10-06).

**Outcome:** a principles catalog lives in the user configuration next to `roles.json`. Roles reference principles by ID. Admission copies the principle text into the assignment brief snapshot.

**Acceptance cases:** editing a principle affects new assignments only; deleting a referenced principle is refused or explicit; the assembled-brief preview includes principle text.

### S7 — Role revisions and the learning loop

**Accepted on macOS 2026-10-06** at `827def7`; see the [progress record](testing/mvp-progress.md#m8-s7--role-history-and-learning-2026-10-06).

**Outcome:** role revisions are kept as an append-only history with diffs. "Ask for a change" and "Teach this role" create operator-originated learning proposals that go through the existing decision path. "New specialist" drafts a role from a description through a short, recorded drafting process run by the runner (like Explore/Grill); the result is a proposal the operator edits before saving. Without a runner, the manual form remains available and the page says drafting is unavailable.

**Acceptance cases:** proposals record their origin (coordinator or operator) and evidence; approval creates exactly one new revision with provenance; a drafting failure leaves no partial role; credentials stay server-side.

### S8 — Playbooks (runner contract)

**Accepted on macOS 2026-10-06** at `c92fe4d`; see the [progress record](testing/mvp-progress.md#m8-s8--playbooks-2026-10-06). All eight slices are accepted within the macOS scope; Linux/Wayland terminal verification remains the approved follow-up.

**Outcome:** a playbook catalog lives in the user configuration. The coordinator proposes a playbook at run start, and the operator can change it before queueing or during a run (sent as steering). The run keeps a checklist copied from the playbook; the coordinator records step progress and skip reasons through a new tool. The session header shows the checklist.

**Acceptance cases:** the coordinator tool contract is tested against a real OMP runner, as `AGENTS.md` requires; a skipped step needs a reason; the checklist never affects readiness; replay after interruption restores the checklist.

### S9 — Verification and evaluated learning

**Status:** scope approved 2026-10-04. S9.1–S9.2 accepted 2026-10-13 and S9.3 accepted 2026-10-14 within the bounded evidence below. The slices were implemented in S9.1 → S9.2 → S9.3 order.

**Outcome:** agents can find and execute the right verification path, recurring corrections lead to demonstrated enforcement, and the operator can compare proposed instructions with the current revision before adopting them. Success is observable correctness and reduced operator effort, not PR count or checklist completion.

**Reuses:** the existing [scenario catalog](testing/scenarios.json), [native acceptance mapping](testing/rust-native-acceptance-reconciliation-2026-10-04.json), [operation inventory](native-workbench-operation-map.md), verification scripts and evidence receipts; retained steering/review/repair history; S3's decision inbox, S6's principle snapshots, S7's learning proposals/revisions and S8's playbook snapshots. Keep one workflow authority, the current runner and the existing admission, ownership and recovery rules.

#### S9.1 — Feature-to-verification map

**Accepted 2026-10-13:** [feature index](testing/feature-map/README.md), [receipt](testing/for-12-feature-map-2026-10-13.json) and [executed evidence](testing/mvp-progress.md#for-12--s91-implementation-and-acceptance--2026-10-13). Three fresh-specialist native recipes plus a corrected stale-selector rerun passed 24 assertions, with checked owned cleanup and independent review. Evidence uses Linux source/debug headless GPUI, scripted RPC/Mail/review and real daemon/process/Git/Docker facts; it does not certify live delivery, OS input or other platforms.

**Deliver:** a repository-local index under `docs/testing/feature-map/` connecting an operator's feature name to prerequisites, navigation and actions, observable outcomes, the existing harness command and retained evidence. Each recipe identifies the supported surface/platform, instance/build identity, isolated setup, readiness check and cleanup. Use versioned helpers around existing controls where needed; do not create another acceptance catalog or require agents to invent a new script per run.

Start with steering delivery across a reconnect, then add pause/resume and readiness invalidation after a revision changes. Reference existing case IDs and commands. The recipe index is part of the verification assignment's discoverable context; include the relevant recipe revision in its evidence. A recipe change is re-exercised on its actual surface before being described as verified.

**Acceptance cases:**

- A specialist starting from the documented feature name can exercise steering across reconnect without undocumented setup coaching; actual user actions, stored/received/applied states and the durable record agree.
- All three recipes execute against isolated Crewroom-owned state and identify the exact application/source revision and platform. A native user journey includes native interaction evidence; a service test or legacy browser run alone cannot satisfy it.
- Unavailable runner, provider or platform prerequisites produce an explicit unavailable/incomplete result. Scripted-provider evidence is labelled separately from live delivery; no skipped action becomes a pass.
- A changed entry point or stale recipe is detected and corrected through a demonstrated re-run. Cleanup stops only resources created for verification and preserves evidence.

#### S9.2 — Corrections become enforceable checks

**Accepted, 2026-10-13:** the [correction ledger](testing/corrections/README.md) and read-only validator link two authentic incidents to existing enforcement. The [acceptance receipt](testing/for-13-corrections-2026-10-13.json) records original historical failure/current success, five artifact mutations, two independently reviewed assertion improvements, bounded runtime and clean owned teardown. The final claim has bound independent recurrence/enforcement review; production/S7 authority remains unchanged. The [implementation plan](testing/for-13-implementation-plan.md) remains the planning record. FOR-14 follows this item.

**Deliver:** an evidence-linked correction record that groups recurring mistakes from retained steering, review and repair history, then identifies the strongest practical remedy: domain/API design, types, static analysis, behavioral tests or guidance requiring judgment. A recurring class cites at least two observed occurrences. Record why the selected layer fits and whether an existing check already covers it.

Guidance changes reuse the reviewed learning path. Structural changes become separately scoped repository work with ordinary revision-bound checks and independent review; recording a correction does not automatically queue that work or edit a role. Keep a compact mapping from the mistake and evidence to the enforcing code/check or approved guidance revision.

Include a bounded mutation-testing trial on one small deterministic module involved in evidence, ownership or message disposition. Inspect surviving mutations for genuine behavioral gaps versus equivalent or irrelevant changes; retain the affected tests, findings and runtime cost. This is a one-module assessment of test strength, not a global mutation-score or complexity threshold.

**Acceptance cases:**

- Trace one recurring mistake from its recorded occurrences through a justified remedy to proof that the check rejects a real historical bad case and accepts legitimate behavior. Reuse existing enforcement if it already works; do not invent a new defect or duplicate a check to complete the slice.
- A source reference that is missing, or an isolated one-off correction, cannot be presented as an evidenced recurring class. A finding requiring judgment remains a proposal rather than an asserted mechanical guarantee.
- The mutation trial records its module/source revision, tool configuration, exercised mutations, outcomes and runtime. Any assertion improvement is verified against both the original behavior and the relevant mutation. A trial that finds no useful gap may complete with that bounded result.
- Proposal review preserves S7's operator decision, provenance and immutable admitted assignments. Structural work follows the existing operator queue and never bypasses the human merge gate.

#### S9.3 — Compare instruction revisions before adoption

**Planning checkpoint (2026-10-14):** the [FOR-14 implementation plan](testing/for-14-implementation-plan.md) and [review page](../.lavish/for-14-implementation-plan.html) map this accepted scope to current native/runtime source, proposed comparison persistence, restricted execution, all three editor paths and bounded live/recovery validation. That checkpoint was planning-only.

**Accepted 2026-10-14:** [comparison contract](testing/for-14-comparison-contract.md), [revision-bound receipt](testing/for-14-comparison-2026-10-14.json) and [executed evidence](testing/mvp-progress.md#for-14--s93-implementation-and-validation-2026-10-14) record all three native editor paths, actual-runner contracts, 16 live evaluated workers, separate recovery, exact whole-state restore and independently reviewed cancellation correction `5292d3e`. Current instructions succeeded in 8/8 cells and proposed in 7/8; the honest incomplete result and failed original matrix wrapper remain retained. Audited durable results establish the full observation gate. No improvement or equivalence is established and no instruction is adopted. Scripted, live and platform limits remain separate.

**Deliver:** an operator-started comparison for one proposed role, principle or playbook revision against its current revision. Reuse the relevant editor/proposal surface for starting a bounded evaluation and opening its result; add no separate fleet or automatic promotion workflow. Begin with one real proposed revision while using the same comparison contract for all three instruction kinds.

Persist the two instruction snapshots, task/requirements and repository revisions, case IDs, runner/tool identities, model/effort, provider mode, repetition count and active-execution limit before starting. Change only the instruction variant under comparison. Each attempt uses isolated disposable state, obeys existing concurrency limits and cannot publish a PR, update Jira or change the operator's configuration as a test side effect.

Use at least four representative cases: a missing/skipped required check, stale revision evidence, a routine reversible decision that should not need escalation, and verification on the correct user surface. Run at least two attempts per case per variant. Workers receive ordinary task prompts without the scoring rubric or variant labels. Score observable artifacts and actual tool use first; any judging agent sees neutral labels, stays independent of the workers, and returns evidence-backed findings rather than a model preference.

Retain per-case correctness and false-completion results, repair cycles, operator interventions, duration and actual usage where available. Report unavailable accounting honestly. Attach the comparison to the exact proposal/revisions; an edited proposal requires a new comparison and leaves the old result historical. An inconclusive comparison remains visible and must not be summarized as an improvement.

**Acceptance cases:**

- Old and proposed instructions execute the same cases with the recorded settings and repetitions. Results retain each attempt and a baseline comparison; a single successful run does not establish improvement.
- Deterministic fixtures prove that false completion, stale evidence and wrong-surface verification are detected. A separate bounded run through real supervised agents and a live model supplies behavioral evidence; scripted replies alone cannot complete that live acceptance case.
- Pause, interruption, restart and unavailable-provider cases preserve completed results and mark unfinished attempts accurately. Resumption uses the existing supervision/ownership contract without duplicating writers or external effects. Any new runner tools pass a real-runner contract check.
- The operator can inspect results and adopt or reject through the existing decision path. Evaluation completion never adopts a revision, changes a current assignment, satisfies coding readiness or becomes a mandatory gate for every ordinary configuration edit.

**Scope limits:** S9 does not change the runtime, concurrency policy, automatic intake policy, durable requirements or human merge decision. It does not add blanket comment deletion, universal coverage/complexity targets, or a general benchmark platform. Existing live-provider and native-surface gaps remain outstanding until their own evidence exists.

**Exit evidence:** link the three executed feature recipes, one complete correction-to-enforcement example, the bounded mutation report and the repeated live instruction comparison from [mvp-progress.md](testing/mvp-progress.md). Each identifies its revision and execution limits. An evaluation that demonstrates no improvement is valid evidence; a missing required run is not. S9 is accepted only after its required artifacts and contracts have been exercised, independently of whether the proposed instructions are adopted.

## Verification

- Domain and service: `cargo test` for `crewroom-core`, `crewroom-service` and `crewroom-runtime`, plus `npm test` where retained domain contracts are touched.
- Native UI: `cargo test -p crewroom-desktop` (GPUI interaction tests), build via `./scripts/run-native.sh --build-only`, and a visual check of the running app on Linux Wayland. The macOS package is checked by the operator before release.
- Legacy browser build: `npm run build` and `npm run test:e2e` stay green; the redesign targets the native workbench only.
- Each slice records its revision, commands and results in [mvp-progress.md](testing/mvp-progress.md).

## Operator decisions (2026-10-04)

1. Keep the agent wall as an optional pop-out window during S2; revisit retiring it after S2 is in use.
2. Store principles and playbooks in the user configuration next to `roles.json`.
3. Take the slices in order S1 → S9. S9 was added after the resource review; S1–S8 retain their existing order and acceptance requirements.
4. On 2026-10-06, commit and push each tested slice directly to `main`; no feature branches are needed for this single-developer project. macOS terminal validation permits continuation, while Linux/Wayland verification remains unverified follow-up work.
5. Retain the approved S9 verification, correction-enforcement and instruction-evaluation work, including one bounded mutation-testing trial. Reuse the catalogs and approval flows already assigned to S5–S8.
