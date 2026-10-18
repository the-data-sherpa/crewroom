# FOR-12 — Feature-to-verification map: implementation plan and testing constraints

Planning checkpoint: **2026-10-13; implementation authorized later that day**. The operator requested this plan before execution, then requested implementation and targeted agents. [FOR-12](https://linear.app/forgeinnovations/issue/FOR-12/s91-feature-to-verification-map) is implemented and validated at `a3a984819b3f5fa31054649d9f1a5920536a9390`; the [receipt](for-12-feature-map-2026-10-13.json) and [status](../status.md) record completion. Planning inspected source: `38b3670a009e9af0c71153a61e0484cc96ba5708` on `main`. FOR-11 is Done; FOR-13 and FOR-14 remain subsequent work.

This plan implements [S9.1](../workbench-redesign.md#s9--verification-and-evaluated-learning) within the existing [product](../product.md), [architecture](../architecture.md), [domain terminology](../../CONTEXT.md) and [reuse assessment](../research/omp-team-reuse-assessment.md). The historical [test strategy](strategy.md) explains evidence layers; [current progress](mvp-progress.md) determines what has actually run. The plan below preserves the accepted implementation choices and testing constraints; the linked receipt contains executed acceptance evidence.

## Outcome and scope

A specialist given a feature name can find a versioned recipe, establish an isolated native instance, exercise its controls, compare visible outcomes to durable facts, retain evidence and clean up without undocumented coaching or a new script for every run.

Exactly three recipes are required, in this order:

1. **Steering across reconnect** — stored, received and applied remain distinct and survive reconnect.
2. **Pause/resume** — pausing lasts until owned execution stops; explicit Resume continues durable history.
3. **Readiness invalidation after a revision change** — a previously ready candidate loses readiness when its source basis changes; old checks and review remain historical.

The map references existing acceptance case IDs. Recipe names are lookup keys, not another acceptance catalog. No workflow schema migration, runner adapter, production control, role revision or new application screen is planned. Small extensions to versioned verification helpers and GPUI action drivers are in scope where existing commands cannot demonstrate the recipe. A product defect uncovered by a recipe is recorded separately unless a bounded fix is explicitly brought into this slice.

**Excluded:** FOR-13 correction classification/mutation trials; FOR-14 instruction comparisons/live model evaluation; repeating MH6 pair jobs or all harness contracts; full Jira-to-PR delivery; new macOS/Windows or packaged release certification; Linux terminal/IME/screen-reader closure; full-state restore; coverage targets; benchmarks; global harness/login changes; automatic acceptance or adoption. Existing unrelated gaps retain their recorded status.

## Inspected reuse and missing connections

| Recipe | Existing case ownership | Reuse at the inspected revision | Work needed for this slice |
| --- | --- | --- | --- |
| Steering across reconnect | `UI-02`; relevant steps of `E2E-05` | `native_conversation_pages_reconnect_and_keyboard_store_paused_steering` in [collaboration tests](../../crates/crewroom-desktop/src/workbench/collaboration/tests.rs); native state presentation in [acceptance tests](../../crates/crewroom-desktop/src/workbench/acceptance_tests.rs); [native intake/restart gate](../../scripts/rust/native-intake.test.ts); [daemon workflow](../../scripts/rust/daemon-workflow.test.ts) and [lifecycle gate](../../scripts/rust/workflow-lifecycle.test.ts) | Current reconnect/intake tests retain pending steering; state presentation seeds receipts separately. Join native input, controlled coordinator unavailability, reconnect, actual fixture receipt/disposition and native readback in one identifiable journey. Do not claim that pending-only or directly seeded applied state proves delivery. |
| Pause/resume | `UI-04`, `LIF-05`; Resume portion of `LIF-06`; relevant steps of `E2E-05` | [native controls gate](../../scripts/rust/native-controls.test.ts), [operator driver](../../crates/crewroom-desktop/src/workbench/journey_tests/operator.rs), [workflow fixture](../../scripts/rust/workflow-support.ts) | Select the Pause/Resume work without importing specialist insertion or PR-feedback publication as mandatory recipe steps. Retain intermediate ownership, bounded active work, explicit Resume and cleanup observations. |
| Readiness invalidation | `EVD-04`; revision-drift portion of `E2E-09` | [native target-refresh journey](../../crates/crewroom-desktop/src/workbench/journey_tests.rs), [evidence gate](../../scripts/rust/evidence.test.ts), [candidate/review fixtures](../../scripts/rust/workflow-coding-fixture.ts), [native evidence view tests](../../crates/crewroom-desktop/src/workbench/lifecycle_tests.rs) | Establish a genuinely ready fixture candidate first, advance its isolated target, use native Refresh target, then prove new revision/false readiness in the native Results view and durable assessment. Existing target-refresh coverage proves false readiness after refresh but alone does not demonstrate the required ready → invalid transition. |

The authoritative references remain [suite.json](suite.json), [scenarios.json](scenarios.json), [native reconciliation](rust-native-acceptance-reconciliation-2026-10-04.json) and the [operation inventory](../native-workbench-operation-map.md). Historical `specified-not-implemented` labels in the original suite do not overwrite later reconciliation. These recipes cover named portions of those cases; completing them does **not** certify all of E2E-05/E2E-09, cancellation, specialist insertion, target conflicts or hosted CI regression.

The [shared native script](../../scripts/test-native-resume.sh) currently builds the driver and runs Resume, controls and intake together; [native journeys](../../scripts/test-native-journeys.sh) runs all matching ignored journeys. Both are reuse sources, not the required per-recipe command. [native-ui-driver.ts](../../scripts/rust/native-ui-driver.ts) already runs a GPUI process against a caller-owned daemon. The fixture already supports `CREWROOM_RETAIN_FIXTURE`, checks daemon exit and inspects owned host/runner descriptors. Preserve those checks when adding retention and selection.

## Planned files and small public interface

| Location | Responsibility |
| --- | --- |
| `docs/testing/feature-map/README.md` | Feature names and aliases → recipe links; supported surface; status and historical evidence links. Explain how to start from a feature name. |
| `docs/testing/feature-map/{steering-reconnect,pause-resume,readiness-invalidation}.md` | Three complete recipes using the contract below. |
| `docs/testing/feature-map/manifest.json` | Machine-readable references to existing case IDs, recipe paths, entry points and relevant source/helper fingerprints. It carries discovery/freshness metadata, not duplicate acceptance definitions. |
| `scripts/verify-feature.py` | One versioned entry point for listing, checking prerequisites, executing one recipe and ownership-checked cleanup. Orchestrates existing harnesses; no parallel workflow state machine. |
| Existing `scripts/rust/` fixtures and GPUI journey drivers | Minimal action, selection and receipt extensions needed by the three recipes. A focused `feature-recipes.test.ts` may compose those helpers if the existing files cannot expose narrow selection cleanly. |
| `AGENTS.md` and normal repository verification context | A short pointer to the index for verification assignments. Use existing captured project instructions; do not alter global reusable roles or admitted assignments. |
| `docs/testing/for-12-feature-map-<date>.json`, `docs/testing/mvp-progress.md`, `docs/status.md` | Final revision-bound receipt and truthful completion/limitations. Historical large/private artifacts remain referenced by hash and retained location. |

Implemented helper interface (see the [feature index](feature-map/README.md) for full prerequisites, explicit build setup and short evidence paths):

```sh
python3 scripts/verify-feature.py list
python3 scripts/verify-feature.py build --evidence-dir <new-build-directory>
python3 scripts/verify-feature.py check steering-reconnect
python3 scripts/verify-feature.py run steering-reconnect --evidence-dir <new-directory>
python3 scripts/verify-feature.py run pause-resume --evidence-dir <new-directory>
python3 scripts/verify-feature.py run readiness-invalidation --evidence-dir <new-directory>
python3 scripts/verify-feature.py cleanup --receipt <retained-run-receipt>
```

`check` is read-only and reports readiness; `run` invokes only the selected journey after successful preflight and finishes with evidence plus cleanup. `cleanup` is an idempotent recovery operation for the named run, never a global process sweep. Every launch is recorded before it can leave orphaned work. A missing prerequisite does not trigger installation, credential refresh, image pulling or fallback to a different surface. Setup/build instructions are explicit in the recipe and use the existing bootstrap/build mechanisms.

Use one receipt writer and source of manifest validation across these commands. Keep executable argument arrays in code rather than shell commands interpolated from recipe text. Support just the three known recipes; a plugin registry, schema framework or general scenario engine is unnecessary.

## Recipe contract

Each recipe must specify all of the following:

- **Discovery:** canonical feature name, useful aliases, existing case/scenario links and the exact subset exercised.
- **Surface:** native GPUI controls backed by the production Rust daemon; first verification on Linux source/debug. Record OS/version, architecture, display/headless mode and toolchain. GPUI harness interaction is native evidence, but is not OS-input, embedded-terminal or packaged-app certification. Other platforms remain unverified until separately exercised.
- **Identity:** recipe commit plus content hash; helper/source fingerprints; application commit and dirty diff hash if applicable; daemon and desktop/test executable paths and hashes; fixture target/candidate head, base and tree as distinct identities. A source HEAD alone does not prove an old executable was rebuilt.
- **Prerequisites:** supported Node version (`package.json` currently requires ≥24), Python, Rust/locked dependencies, Zig/native libraries, required workspace/example/test binaries; tmux/process ownership helpers and Docker/image only where the selected journey uses them. State runner/provider mode explicitly. Use a narrow prerequisite set per recipe.
- **Isolation:** generated run ID, new private fixture/config/database/Git paths, loopback endpoints, private connection descriptor, fixture-specific tmux socket and resource ownership records. Require a fresh evidence directory; never overwrite a previous attempt. Use synthetic tickets and provider credentials, not the active installation or operator repository.
- **Readiness:** verified executable/source binding, exactly one expected test selected, healthy authenticated daemon and native connection, correct fixture ticket/run, controlled runner/provider ready or deliberately held at the recipe's named step. Never infer readiness from an empty log or a sleeping process.
- **Actions and outcomes:** exact navigation labels/control IDs and input text, expected intermediate/final UI states, durable record IDs and assertions. Identify which setup/fault operations use helpers and which are native user actions. Do not seed the receipt or transition being claimed as observed.
- **Evidence and cleanup:** exact command/arguments, start/end/exit/timeout, expected and executed assertion/test counts, native action trace and rendered/control observations, durable before/after snapshots, sanitized provider/process facts, failures, cleanup ownership/exit results and retained artifact hashes/paths.
- **Limits:** unavailable/failed/stale/incomplete outcomes with actionable reason and next step; no skipped action becomes a pass. Instructions for safe rerun and source changes are explicit.

Receipt outcome vocabulary: `passed`, `failed`, `unavailable`, `incomplete`, `stale`. Only `passed` with every required assertion and successful owned cleanup satisfies a recipe. An expected negative preflight may be a passed **guard check**, while the journey itself remains unavailable/stale. Scripted runner/model/Mail/provider facts are labelled separately from actual daemon, UI, Git and OS-process facts; no live delivery claim follows from a scripted exchange.

## Three bounded journeys

### 1. Steering across reconnect

Start with a connected native workbench and an isolated coordinator whose delivery is deliberately held by an existing fixture latch. Select the ticket, open Conversation, enter a distinctive instruction and activate Send (`save-steering` or its current keyboard action). Assert one durable steering ID/body and pending/stored presentation. The daemon must be reachable to store an instruction: do not claim successful offline submission while the authority itself is disconnected.

Reconnect the native client (and exercise the documented service reopen only if used by the recipe), inspect the same stored record, then release coordinator delivery. Capture received before requesting a fixture `steering_receipt` applied disposition through the authenticated host tool. Inspect Conversation again and compare IDs, times and disposition with the durable record. Reopening must not duplicate the instruction or its application. A provider reply saying “done” is insufficient. Use bounded latches rather than sleeps to make pending/received/applied individually observable.

### 2. Pause/resume

Start documented bounded active fixture work; retain its ownership descriptor and progress artifact. Activate native Pause (`pause-run`), observe pausing while ownership is unstopped, then paused only after the daemon confirms the actual owned processes/descendants stopped. Reopen and compare the same run/history/artifact. Activate native Resume (`resume-run`), verify continued work and current attempt/process identities with no simultaneous old writer, then stop via owned cleanup. Report replacement attempts honestly rather than requiring an unsupported same-session recovery claim.

Reuse the native control gate's process observation and the existing command fixture for a bounded child where needed. Do not add busy specialist insertion, PR feedback, external publication or a new runner matrix to this recipe.

### 3. Readiness invalidation after a revision change

Prepare a disposable repository candidate with recorded successful real fixture checks and an independent scripted review bound to its revision. Inspect ready status in native Results. Advance the disposable target with a nonconflicting fixture commit, then activate native Refresh target (`target-refresh`) under stopped ownership. Assert the changed head/base/tree, retained old candidate/check/review history, explicit stale/missing evidence and `ready=false` in both native results and the durable assessment. Old success cannot satisfy the new candidate; no agent restarts automatically.

The positive starting state is mandatory; a fixture that was never ready cannot prove invalidation. This recipe ends at verified invalidation. Reimplementing the fix, restoring readiness, handling merge conflicts, updating a PR or exercising late hosted CI is outside its bounded purpose.

## Implementation sequence and gates

| Step | Deliverable | Gate before advancing |
| --- | --- | --- |
| 1. Freeze scope and bindings | Refresh FOR-12, confirm FOR-11 Done; inventory exact case IDs, native selectors and existing commands. Create index, manifest and recipe skeletons marked unverified. | Each of the three features has one discoverable entry, correct case ownership and explicit limits. |
| 2. Establish the helper contract | Implement narrow list/check/run/cleanup, source/binary binding, isolation, retained evidence and expected-test-count enforcement. Reuse existing process cleanup. | Missing dependency, stale entry point, zero-selected test and foreign cleanup target produce truthful non-passes without starting unrelated work. |
| 3. Complete steering first | Join native input/reconnect with fixture receipt/application; write exact operator steps and diagnostic locations. | One native steering journey shows the same ID across stored/received/applied and reconnect, with verified owned cleanup. |
| 4. Add pause/resume | Wrap the existing controls/ownership path; retain active work and intermediate state. | One isolated native sequence proves stopped-before-paused and explicit safe Resume. |
| 5. Add revision invalidation | Compose a ready candidate, real fixture target movement, native refresh and evidence readback. | One native sequence proves ready → invalid against distinct revisions, retaining historical evidence. |
| 6. Prove discovery and freshness | Add the project-instruction pointer; run the three recipes from their feature names with a fresh specialist; run the stale-recipe drill below. | All three are executable without private notes/coaching, and a stale entry point is detected then corrected and re-exercised. |
| 7. Close with bounded evidence | Run applicable changed-code checks once, inspect acceptance/cleanup receipts, record limitations and exact commits, update Linear and repo records. | All FOR-12 acceptance items met on the recorded native surface; final issue/comment readback confirmed. Only then Done. |

For step 6, assign a fresh verification specialist only the feature name, repository checkout and the normal documented project context. The specialist can consult the index and linked instructions. Retain the initial assignment, commands, discoveries, questions and result. Do not supply private setup answers during a run: a needed answer identifies a documentation defect. Amend the recipe and rerun the affected journey with fresh context. One specialist may run all three; extra agent teams or three live model/provider jobs are not required. This is a later implementation acceptance activity, not part of preparing this plan.

## Testing constraints

### Required slice evidence

| Check | Minimum and purpose | Explicit boundary |
| --- | --- | --- |
| Static map checks | All three recipes; valid existing case IDs and paths; required contract fields; discoverable instruction pointer; command/filter existence. | No new comprehensive acceptance catalog or coverage percentage. |
| Helper guard checks | Focused checks for unavailable prerequisite, stale entry point, zero tests, mismatched binary identity, interrupted cleanup/evidence retention and refusal of foreign cleanup resources. | Exercise the public helper interface. Combine equivalent cases; do not mirror internal functions with tests. |
| Native journeys | One accepted fresh-specialist execution per recipe on the final applicable recipe/helper revision. Earlier author runs may diagnose but cannot replace the discovery test. | Exactly three journeys, sequentially. GPUI interaction and durable assertions are required; service-only or legacy browser evidence is insufficient. |
| Stale-recipe drill | One intentional stale entry point, detected as stale; correction followed by a native rerun of that recipe. | Change a disposable copy of the recipe/manifest to name a nonexistent selector. No production-code mutation or broad mutation campaign. |
| Cleanup | Every journey, including failed/interrupted attempts. | Stop only receipt-owned daemon/UI/runner/Mail/container/tmux resources; verify stopped identities and daemon exit; keep evidence. Unresolved ownership makes the run incomplete. |
| Final review | Trace every FOR-12 acceptance item to receipts and inspect the final diff for expanded scope and false provenance. | Preserve independent review where code is changed. No requirement to rerun completed MH slices. |

A missing prerequisite is checked once before launching a journey. Record the dependency, detection command and availability result. Classify an unexpected tool timeout as incomplete/failed, not a product pass. Disable a prerequisite in an isolated guard fixture to test this handling; do not alter global installations or logins. If the selected environment cannot execute a required native action, leave acceptance incomplete; substituting service or browser coverage does not close it.

**Freshness rule:** resolve paths and enumerate tests/controls before execution; fail on zero matches or unexpected selection. Record recipe content and relevant helper/driver source hashes in each receipt. A change to a recipe, its commands/selectors or relevant helper/driver makes that recipe unverified until rerun on the actual native surface. A shared helper change invalidates all three affected recipes. Unrelated documentation edits do not invalidate binaries or trigger full application reruns; retain the precise binding rather than insisting every receipt use the last documentation-only commit.

**Stale drill sequence:** preserve a known recipe revision → make a disposable stale selector copy → observe `stale`/zero-start failure and retain it → correct the documented selector/manifest → have the specialist rerun that recipe through native interaction → retain both failed and corrected identities/results. A static link fix alone does not close this acceptance item.

### Changed-code regression selection

These are the accepted implementation-time gates; they were not run for the earlier planning-only commit. The final receipt records exact executed commands and counts. Build setup belongs in the versioned helper/recipe; never rely on a private `/tmp` script or undocumented shell environment.

| Change made during implementation | Required validation |
| --- | --- |
| Markdown/index only | Reference/case/manifest checks and required recipe executions; no application suite just for prose. |
| Python selection/preflight/cleanup helper | Focused behavior tests of the helper's guard contract; Python syntax check; actual selected native journey. |
| Rust GPUI driver/test changes | `cargo fmt --all -- --check`; `cargo build --workspace --locked` (and `--examples` when those fixtures are used); affected desktop tests with explicit filters/selection counts; the three recipes at their final applicable revisions. |
| Rust domain/runtime/service logic (unexpected) | Reassess scope first. If explicitly included, `cargo test --locked -p crewroom-core -p crewroom-runtime -p crewroom-service -- --test-threads=1`, appropriate build/Clippy and `npm test` per repository contract. Add only regression evidence relevant to the defect. |
| TypeScript/build changes | `npm run build`; affected `node --import tsx --test --test-concurrency=1 ...` fixture tests after documented native build/preflight. |
| Domain changes | `npm test` as required by `AGENTS.md`, plus affected native contracts. |
| User-flow changes | `npm run test:e2e` per `AGENTS.md`, plus native journey proof. If only verification documentation/drivers change and production flows do not, do not add the legacy full browser gate by default. |

Current existing commands/filters for reuse include `cargo test -p crewroom-desktop --locked native_conversation_pages_reconnect_and_keyboard_store_paused_steering -- --test-threads=1`, `native_external_operator_control`, and `native_journey_target_refresh_and_reasoned_conflict_retry_keep_revision_history`. The latter two require their documented fixture/build setup (and ignored-test selection); they are not standalone copy-and-run recipe commands. Publish final narrow commands only after they actually work from the recipe.

### Rerun and stop rules

1. Plan for **three accepted native recipe runs plus one corrected stale-recipe rerun**. This is the completion matrix, not permission to hide diagnostics or a cap that waives missing acceptance.
2. A failed run gets a recorded classification: recipe/documentation defect, helper defect, environment unavailable, product regression or unresolved ownership. Preserve its logs and source identity. Name the hypothesis before another run.
3. Fix an in-scope recipe/helper defect and rerun only affected recipes/checks. A shared helper fix requires all affected recipes; a source or selector change cannot inherit a previous pass. No unconditional “full suite again” after a documentation receipt update.
4. After two unsuccessful reruns of the same classified failure, stop that lane and report evidence, attempted fixes and next bounded action. Work on independent documentation may continue. Further runs need new evidence or an explicit revised testing decision; never increase deadlines repeatedly to obtain a pass.
5. Keep existing bounded waits/runner timeouts unless measured evidence justifies a named adjustment. Record selected action/whole-run deadlines in the helper receipt. Time spent waiting without a completed assertion proves nothing.
6. Missing platform/provider/runner prerequisite, ambiguous owned resources, or a required production/schema/adapter redesign stops dependent acceptance. Retain FOR-12 active/incomplete when implementation is underway; record a CrewRoom follow-up for work outside this scope and link the blocking acceptance item. Do not silently expand into it.
7. Do not start paid live-model, real Jira/PR, whole-state restore, release packaging or cross-platform testing as diagnostics for these recipes. FOR-12 requires honest scripted-versus-live labels, not a new live-delivery certification.

## Completion and handoff

The final receipt binds the recipe/index/helper revisions to application/binary identities, platform, case subsets, native actions, durable outcomes, freshness failure/correction, fresh-specialist execution and checked cleanup. No artifact is counted solely because a process exited zero. A mismatched binary, omitted native action, unavailable dependency, lost evidence or incomplete cleanup leaves that recipe unverified.

Before closing, refresh FOR-12 and comments, reconcile new operator decisions, update this plan only where accepted choices changed, record exact implementation/evidence commits and check outcomes in Linear and [status](../status.md)/[progress](mvp-progress.md), then fetch Linear again to verify state/project/content. Keep large raw evidence in a documented retained location with hashes; publish only sanitized receipts. The retired OMP installation, active user installation and unrelated fixture resources stay untouched.

**Completion checkpoint (2026-10-13):** implementation and native recipe acceptance passed at `a3a984819b3f5fa31054649d9f1a5920536a9390`. A fresh specialist discovered and ran all three recipes without private coaching, then detected a stale selector and passed its corrected native rerun. The initial long socket-path failure and the bounded helper/documentation repair remain recorded. The [receipt](for-12-feature-map-2026-10-13.json), [status](../status.md) and [progress](mvp-progress.md#for-12--s91-implementation-and-acceptance--2026-10-13) hold validation, independent review and limits. Later ledger-only edits do not replace the tested source identity.
