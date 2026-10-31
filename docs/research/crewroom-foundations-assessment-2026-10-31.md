# Crewroom against the software factory foundations

Assessment date: 2026-10-31. Tracking: FOR-96. Inspected application revision: `bd5561462744fa39fef42b61070a896bb4dda143` on `main`. Application source was unchanged during this assessment. This assessment applies the [independent, consensus-reviewed research](software-factory-foundations-2026-10-31.md); it is not itself a new Opus consensus review. Recommendations below are proposed, not changes to the accepted product contract.

## Finding

**Crewroom has substantial foundations. The next step should be consolidation around one dependable, useful operating path, rather than rebuilding the application or adding another orchestration layer.**

The accepted product is already narrower than a complete software factory: a personal workbench that carries understood work through supervised agents, verified changes and a reviewable PR, with human merge authority. Current source contains meaningful enforcement of that boundary. Plans must cover accepted criteria; implementation cannot approve itself; evidence belongs to a candidate revision; uncertain process ownership and publication effects hold progress. These are foundations worth preserving.

The concern about building too much has a more precise formulation: **Crewroom has implemented advanced coordination, provider choice, project scheduling and instruction experiments before establishing a current, repeat-use account of delivery reliability and total operator effort.** This is a sequencing and evidence gap. It is not proof that those features are unnecessary. Many were explicitly requested by the operator.

The largest gaps are:

1. A fresh, integrated demonstration on the actual intended operating path, followed by representative ordinary use.
2. Measurement of total human effort, including setup, supervision, review, failed attempts and recovery.
3. Evidence that the mandatory team structure earns its additional coordination cost.
4. Explicit proof against misuse of an otherwise permitted publication or reporting action.
5. A demonstrated handoff from the PR to release/use and feedback, owned by people or existing tools where appropriate.

## How to read the evidence

**Demonstrated** means demonstrated within a stated scope, not certified for all configurations. **Partial** means important mechanisms exist but the foundation has an outstanding part. **Unverified** means the reviewed evidence does not establish the claim; it does not mean the feature is absent. **Missing** is reserved for an identifiable absent capability. **External** means deliberate ownership outside the app; a working handoff must still be shown.

This assessment read the accepted product, architecture, domain vocabulary, current tracker context, representative source paths, tests and retained receipts. It did not inspect private runtime stores or credentials. It is not an exhaustive code/security audit or a fresh full-system acceptance run.

Fresh checks at the inspected revision passed **72 tests**: 55 core tests and 17 runtime tests. These cover intake, choreography, capacity, comparisons, backend evaluation permissions, simulated GitHub publication/delivery, and real local process ownership. They do not establish live model quality, native UI usability or a complete live delivery journey. Exact commands and limits are in the [assessment check receipt](../testing/for-96-foundations-assessment-2026-10-31.json).

The current `pause-resume` feature-map preflight was **unavailable**, first because the shell selected Node 22, then because no verified build receipt existed after selecting supported Node 26. The native recipe was not run. This is an environment/verification gap, not a failing pause implementation. No application code changed, so full build, browser and native acceptance suites were not rerun for this documentary assessment.

Retained live evidence matters, but its revision and scope matter too:

- The [status record](../status.md) records an earlier live Jira-to-PR pilot and human merge. It would be incorrect to say Crewroom has never delivered a real change.
- [MH6 evidence](../testing/mh6-pair-2026-10-13.json) records real inference, supervised pair work, documentation handoff and interruption in bounded disposable jobs. This establishes feasibility within that scope, not the general economic advantage of a pair.
- [FOR-46](../testing/for-46-github-publication-2026-10-18.json) records actual GitHub publication and continuation. Its live probe drove the production host directly and used a plain Git push; it was explicitly **not a full Crewroom run**. Several failure/hosted-check cases were simulated only.
- [FOR-14](../testing/for-14-comparison-2026-10-14.json) retains 16 live worker observations: current instructions succeeded in 8/8, proposed instructions in 7/8; no instruction improvement or general operator-burden reduction was established. Original wrapper failures and historical revision limits remain recorded.
- Earlier private-history receipts were sanitized for publication. They are historical evidence, not byte-identical evidence for current public source; see the [publication record](../publication-review.md).

## Comparison with the sixteen research findings

| Research foundation | Crewroom assessment | Evidence and smallest remaining step |
|---|---|---|
| **C01 — Factory versus factory app** | **Demonstrated scope; external delivery responsibilities** | [Product](../product.md) defines one operator, supervised ticket-to-PR work and human merge. It need not implement deployment or imitate a no-human-review factory. Name and exercise the downstream release/use owner for the chosen path. |
| **C02 — Bounded purpose and authority** | **Demonstrated contract; outcome unverified** | The product, [domain model](../../CONTEXT.md) and [architecture](../architecture.md) distinguish ticket state, workflow, agent activity and authority. The original narrow pilot has expanded to more sources, repositories and harnesses. Choose one current change class and a concrete definition of useful completion for the consolidation trial. |
| **C03 — Acceptance before work** | **Demonstrated mechanics; partial evaluation independence** | [Plan validation](../../crates/crewroom-core/src/intake/plan.rs) requires every criterion to be covered; [handoff](../../crates/crewroom-core/src/intake/requests.rs) requires a current accepted plan and is replayable. [Review](../../crates/crewroom-runtime/src/evidence/reviews.rs) rejects contributing assignments/sessions. These controls do not establish that criteria or tests are sufficient. Add representative behavioral acceptance and review changes to the acceptance basis. Protected comparison checks are not automatically hidden scenarios. |
| **C04 — Whole delivery loop** | **Partial; substantial historical demonstrations** | Intake, work, checks, review, publication and continuation exist. Historical pilot and GitHub host evidence have different boundaries. Demonstrate the selected current path end to end, then observe the delivered result in use. Do not infer whole-system success by joining unrelated passing receipts. |
| **C05 — Reproducibility, small batches and WIP** | **Demonstrated controls; partial current reproducibility** | [Feature-map recipes](../testing/feature-map/README.md), declared builds and isolated fixtures exist. [Capacity](../../crates/crewroom-core/src/capacity.rs) enforces project slots; fresh capacity tests pass. Missing local build receipt prevents today's native journey run. Build and verify the selected path from documented inputs; keep trial WIP within human review capacity. |
| **C06 — Security and operator attention** | **Partial; specific adversarial proof unverified** | External tool authority, server-side credentials, check containers and backend purpose gates are substantial. Fresh [comparison permission tests](../../crates/crewroom-runtime/tests/comparison_security.rs) pass. Existing [scenario E2E-11](../testing/scenarios.json) addresses malicious inputs and forbidden effects. Extend evidence to hostile text attempting leakage through a permitted PR/comment/report destination, and measure the attention needed to supervise it. No exploit is claimed by this assessment. |
| **C07 — Durable state and effects** | **Demonstrated components; integrated recovery unverified today** | [Recovery](../../crates/crewroom-runtime/src/supervisor/workflow/recovery.rs), [publication dispatch](../../crates/crewroom-runtime/src/publication/dispatch.rs), process ownership and retained receipts distinguish intent from confirmed effect. Fresh delivery tests include lost-reply/restart cases; real local ownership tests pass. Run the published pause/reconnect/readiness recipes on a verified build, retaining cleanup receipts. Native monetary enforcement remains limited by accounting capability. |
| **C08 — Operations and feedback** | **Partial; deliberate external ownership downstream** | [Operations documentation](../operations.md) covers installation, backup/recovery and retained state; human PR feedback continuation exists. End-product release, rollback and support can remain external. Demonstrate who owns them and how an observation becomes the next work item. Retention/archive maintenance is also explicitly unfinished in operations documentation. |
| **C09 — Evidence-gated build path** | **Partial alignment** | Crewroom has implemented capabilities across all stages, including later reuse/scale, while some integrated operating evidence remains bounded or historical. Return to Stage 1–3 acceptance on a chosen path, interleaving reliability and actual agent behavior. Stage numbers are not an instruction to delete later-stage code. |
| **C10 — Simplest adequate workflow** | **Policy tension requiring measurement** | [Choreography policy](../../crates/crewroom-core/src/choreography/policy.rs) requires writer, navigator, documentation owner and independent reviewer for every package, with at least four scheduled assignments. This is an explicit accepted design. Its minimum overhead has not been justified by a simpler-path comparison in the reviewed evidence. Preserve it for the initial trial; changing this requirement needs a product decision. |
| **C11 — Evaluate the model and harness together** | **Demonstrated infrastructure; partial task coverage** | [Instruction comparisons](../testing/for-14-implementation-plan.md) freeze inputs, preserve unsuccessful attempts, score final behavior and separate adoption from evidence. Actual runner contracts distinguish scripted compatibility from live inference. Use this machinery where it fits, but evaluate the ordinary product workflow and mandatory safety separately. Passing unit tests or metadata discovery does not prove a live model profile. |
| **C12 — Multiple agents need net benefit** | **Feasibility demonstrated; net benefit unverified** | MH6 proves bounded collaboration; independent review identities are enforced. Neither role count nor agreement establishes greater correctness. A matched comparison must include coordination, handoff failures and human effort, with the same mandatory safety and review gates. |
| **C13 — Outcome and total cost** | **Partial instrumentation; overall value unverified** | Comparison results retain interventions, repairs, duration and usage availability. Product usage distinguishes estimates and subscription information. No reviewed evidence establishes net time saved across ordinary ticket intake, supervision, final review, recovery and later rework. Start a small outcome ledger; a new analytics dashboard is unnecessary. |
| **C14 — Reuse before generalization** | **Substantial reuse; breadth deserves a pause** | [OMP reuse assessment](omp-team-reuse-assessment.md) explicitly preserves durable ownership/evidence patterns without restoring the retired runtime. Git, providers, harnesses, mail and containers supply existing capabilities. Multi-harness parity, advanced scheduling, more configurable stages and learning features should expand when the chosen operating path exposes a constraint. |
| **C15 — Compare against accepted boundaries** | **This assessment supplies the comparison** | Each finding distinguishes feature presence, evidence scope and external ownership. Proposed owners and acceptance gates follow below. Existing tracker work is referenced rather than relabeled as missing implementation. |
| **C16 — Keep uncertainty visible** | **Strong evidence practice, with documentation friction** | Receipts distinguish simulated/live behavior, stale revisions, incomplete work and unavailable usage. Current status also accumulates older continuation guidance, including a single coding slot despite accepted per-project capacity. Add a concise current operating summary; preserve historical receipts and their limits. |

There is no basis here for calling the core workflow foundations wholly missing. The strongest negative findings are unverified integrated reliability/value and policy choices whose benefit has not been established. Those require different remedies from missing code.

## Where Crewroom sits on the build path

| Research stage | Current position | Next gate |
|---|---|---|
| **0. Purpose, owner and boundary** | Defined; later scope has broadened. | Select one current work class, repository, source and execution profile. |
| **1. Thin complete delivery loop** | Historical live proof and current component implementations. | Current integrated run through PR feedback and downstream use, with revision-bound receipts. |
| **2. Dependable execution** | Extensive controls and tests; operational checks span revisions/platforms. | Fresh selected recovery/security checks, truthful holds, stopped ownership and cleanup. |
| **3. Bounded AI value** | Live task execution and instruction experiment infrastructure. | Repeated ordinary work showing quality and total human effort against a baseline. Interleave with Stage 2. |
| **4. Expand authority** | Automatic execution/publication within a defined boundary; human merge remains. | Do not add authority until the current boundary is evidenced, including permitted-action misuse. |
| **5. Reuse and scale** | Multiple harnesses/sources/projects, concurrency and instruction comparison already implemented. | Expand only where observed queueing, portability or repeated task evidence justifies it. |

This is uneven maturity, not a percentage-complete score. Implementing Stage 5 features does not close a Stage 3 value question.

## Recommended next work, in order

### 1. Define and prove one current operating path

**Type:** operating decision plus validation. **Proposed owners:** operator selects the path and acceptance; engineering records evidence.

Use one repository, one ticket source, one approved harness/model profile and one small class of ordinary changes. Run one active coding ticket for this trial; this is a trial constraint, not removal of per-project scheduling. Retain the currently accepted team and all existing evidence gates.

First obtain a verified build receipt and use the feature-map recipes with isolated state. Then carry a bounded representative task through accepted criteria → implementation → checks → independent review → PR → explicit feedback continuation. Establish the human merge/release/use handoff. Record every stop, failed attempt, unavailable step and manual intervention. Do not substitute a direct provider call for the integrated product journey.

**Exit evidence:** a named revision, actual final artifact/PR, matching check/review receipts, observable ownership cleanup, and one observation from intended use that informs a next decision. A sandbox run establishes mechanics; ordinary use is still needed to establish value. No arbitrary number of successful tickets is proposed as a universal threshold.

### 2. Add an outcome ledger and test the team assumption

**Type:** measurement first; any workflow-policy change is a separate decision. **Proposed owners:** operator records effort and outcomes; engineering captures machine evidence.

For each selected task, record accepted outcome, total elapsed time, active operator minutes for setup/steering/review/recovery, interventions, repair cycles, failed/cancelled attempts, model usage with availability labels, and post-delivery defects/rework. Compare with how the same class of task is handled today. Use a document or CSV initially.

Once a baseline exists, test whether the mandatory pair/documentation arrangement improves outcomes enough to justify its overhead. A simpler variant must retain required checks, independent review and authority controls; it cannot presently be selected by merely omitting roles because policy rejects that schedule. Changing the accepted policy or building an isolated experimental variant is follow-up implementation, not part of this assessment. The existing instruction-comparison engine is useful infrastructure, but it is not already proof of a team-topology comparison.

**Exit evidence:** a reasoned retain/simplify decision using matched task conditions and unsuccessful attempts as well as successes. If the sample is small, report descriptive results and uncertainty, not statistical superiority.

### 3. Close the selected path's authority and recovery evidence

**Type:** validation, followed by implementation only for observed gaps. **Proposed owner:** runtime/security maintainer with operator-defined permitted actions.

Keep existing forbidden-action tests. Add a synthetic adversarial case in which untrusted ticket/repository/peer content tries to place a benign secret sentinel into an otherwise allowed PR or reporting payload. Check account, destination, operation and payload policy; an approved domain or permitted tool name alone is insufficient. Check for the absence of the external effect, not just a model refusal. Do this in an isolated fixture or explicitly scoped sandbox, never by exposing real data.

Exercise pause/reconnect/readiness invalidation and uncertain-effect reconciliation using existing recipes. Measure how much operator interpretation these holds require. This complements the controls already present; it is not a claim that they are ineffective.

**Exit evidence:** required safety conditions pass independently of task quality, with attributable denials, successful legitimate actions and verified cleanup. Fix any exposed defect before expanding authority.

### 4. Make the downstream handoff explicit

**Type:** human/external process, with app changes only if a demonstrated handoff fails. **Proposed owner:** operator or repository/release owner.

For the chosen project, identify who merges, how the accepted change reaches intended users, how failures are detected and recovered, and how support/user feedback becomes new work. The app can continue to stop at a verified PR. A checklist and links to existing CI/release processes may be sufficient.

**Exit evidence:** a delivered revision observed in use and a traceable feedback decision. Do not add a custom deployment platform to satisfy a responsibility already handled elsewhere.

### 5. Consolidate status and constrain further expansion

**Type:** documentation and prioritization. **Proposed owners:** maintainer and operator.

Put the selected supported path, fresh evidence revision, remaining operator checks and next gate at the top of status. Reconcile stale continuation statements against current accepted capacity/settings. Keep historical receipts intact. Complete relevant outstanding native operator checks before adding more settings or dashboards.

Existing tracker items should keep their boundaries: **FOR-67** is a live GitHub changes-requested/approval check awaiting a second reviewer identity, not missing review-policy implementation. **FOR-80** proposes project spending controls and still needs scope; reliable metering constrains what can be guaranteed. **FOR-81** proposes configurable ordered review stages; it is not a substitute for the independent review and readiness controls already present. None of these should automatically outrank proving the selected ordinary workflow.

**Exit evidence:** one current operating summary and a short next-work sequence. No automatic reprioritization, policy changes, new follow-up issues or app implementation were made by this assessment.

## What to preserve and what to defer

Preserve accepted plans and immutable run context; separate process ownership; external message receipts; isolated repository work; actual required checks; independent revision-bound review; publication reconciliation; explicit human merge; and honest unknown/incomplete states. Removing these to make the app appear simpler would remove the foundations the research recommends.

For the consolidation trial, defer additional harness/provider breadth, higher concurrency, configurable approval chains, richer dashboards and automatic instruction adoption unless a concrete obstacle requires them. Existing features can remain available. The practical reset is to narrow what we rely on and prove, then expand from observed need.

**Recommended immediate objective:** make one small class of work dependable, measurable and useful through the current Crewroom path. The research argues for that sequence; the repository evidence gives us a substantial starting point rather than a reason to start over.
