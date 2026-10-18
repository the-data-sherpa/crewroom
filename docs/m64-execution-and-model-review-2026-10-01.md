# M6.4 execution and model-role review

Date: 2026-10-01

Scope: Recent session outcomes, JEV observations, M6.3 inefficiencies, and recommendations for M6.4.

This is a retrospective and proposed execution plan. It does not approve product changes, change model configuration, or replace [current implementation status](status.md). Findings reflect the sessions and configuration inspected during this review; active M6.3 work may have advanced afterward.

Revised on 2026-10-01 after checking [Claude's adversarial response](m64-review-adversarial-response-2026-10-01.md) against the session, routing implementation, review policy, and current configuration. The historical JEV counts below retain their original cutoff.

Post-run update added on 2026-10-01 after final acceptance and local commits. The completed-run section supersedes earlier statements about Slice 2 being open or the implementation remaining uncommitted. Workflow and configuration recommendations remain proposals.

Scope-control follow-up, 2026-10-01: the [next-slice plan](mvp-plan.md#m63-slice-3--pr-feedback-continuation), [failure model](architecture.md#m63-slice-3-failure-model), status pointer, and handoff now contain the bounded PR-feedback outcome, acceptance cases, cumulative repair limit, and single verification owner. These next-run instructions are recorded; Slice 3 implementation and the broader JEV/model-configuration changes have not started in this review.

## Main finding

The largest avoidable source of delay in M6.3 was discovering fundamental recovery and ownership requirements during repeated repairs. Model availability and provider failures added friction, but changing models alone will not prevent a repeat.

The remaining M6.3 work and M6.4 need committed acceptance boundaries, explicit failure models before implementation, and a repair budget that follows the acceptance outcome across worker replacements. The model-role table largely restates the existing configuration. The more significant model issue was that the M6.3 lead ran with a session selection that bypassed the intended coordinator routing; the journal does not establish who initiated that selection.

## Completed-run update and JEV assessment

### Accepted result and source identity

M6.3 Slice 1 is committed as `1153f8a`, Slice 2 as `38b62b0`, and acceptance records as `dec01c7`. The final Slice 2 receipt records **262/262 domain tests, a passing build, 18/18 E2E tests, independent Fable recheck, and accepted browser evidence**. The verifier's resources were cleaned up. This is local fixture and process evidence, not a live-provider delivery claim. **PR-feedback continuation remains open.**

This post-run review checked that the Slice 1 commit diff reproduces its recorded acceptance hash exactly. Comparing all 274 paths in the Slice 2 commit against the retained validation snapshot found differences only in `docs/mvp-plan.md`, `docs/status.md`, and `docs/testing/mvp-progress.md`. The tested implementation, tests, and architecture content match. Tests were not rerun for this retrospective.

The routing journal also records an actual correction after acceptance: `/route auto` was selected at 13:15 UTC, and the commit request ran with worker routing and **Opus 5.5 medium**. That correction does not retroactively change the model used during implementation. Preserve the startup check for subsequent sessions.

### Final shadow totals

The frozen global log contains 648 records. Of these, 112 belong to the reviewed session: 111 through the implementation/acceptance period and one subsequent directory-listing request.

| Window | Observations | Comparable next-step assignments | Action matches / differences |
| --- | --- | --- | --- |
| Implementation through acceptance | 111 | 90 | 44 / 46 |
| New observations after the original cutoff, through acceptance | 26 | 26 | 17 / 9 |
| Whole session, including the later directory listing | 112 | 91 | 44 / 47 |

The 21 noncomparable records include other event types and unavailable next-step decisions. These counts compare JEV with the lead's declared assignment, not with ground-truth correctness or actual execution. The full saved report counts 25 complete assignment matches and 66 differences across action, role, and continuity.

Through acceptance, JEV reported 252,076 input and 16,996 output tokens, with 20.191 seconds of summed recorded evaluation latency. These exclude lead-side state construction, repeated guidance reads, context overhead, and unreported usage. They cannot establish billed cost or net savings. The recorded evaluation latency alone does not explain the hours of delay.

The read-only pilot is still **0/20 dispatched**. Its 540 events are all ineligible: 501 `outside_readonly_fresh_lane`, 27 `not_evaluated_next_step`, and 12 `independent_assignment_outside_lane`. These broad reason codes do not by themselves establish why every candidate failed eligibility. There is no pilot execution outcome to promote.

### Outcome-linked observations

Nine saved cases were checked against the subsequent native actions and final evidence. These are retrospective judgments, not an independent benchmark or a replay experiment.

| Frozen log line | JEV recommendation | Evidence-based assessment |
| --- | --- | --- |
| 622 | Wait while review continued | The completed diagnosis was already readable. The lead read it without interfering with the reviewer. Waiting was unnecessary for that independent step. |
| 628 | Wait for completion | The implementer was paused awaiting instructions, and the lead had to resume it. The reported state did not adequately distinguish retained ownership from actively progressing work. |
| 630 | Reuse the reasoner | Appropriate: a concrete legacy-migration uncertainty remained. The retained reasoner resolved it, and the resulting correction was covered by acceptance evidence. |
| 633 | Recover the environment | Conditional: inspecting the pending test once was sensible; presuming a hang or restarting it was not. The next observation confirmed progress, and the existing run finished. |
| 635 | Review with fresh context | Review was needed, but the input named a suitable retained independent reviewer. Reusing that reviewer produced the final recheck without another handoff. |
| 636 | Reuse the verifier | Appropriate after focused checks and independent review; the retained verifier supplied the final accepted evidence. |
| 642 | Repair with the existing producer | Appropriate for the missing local E2E target configuration. Production behavior stayed fixed; final E2E passed and earlier unaffected domain/build evidence was reused. |
| 645 | Investigate a screenshot-format discrepancy | Checking the evidence was appropriate; `investigate` versus `verify` is largely a label difference here. A direct `file` check settled it, so the proposed standard-role handoff was unnecessary. |
| 648 | Fresh Sol-medium investigation | Excessive for the complete request `ls -lrta`. The lead ran the command directly. This later request is excluded from implementation-window totals. |

The final closeout therefore supports retaining focused tests, scoped rechecks, existing worker context, and unaffected evidence. It does not show that JEV caused those decisions: the lead chose independently and JEV remained advisory.

### JEV changes to include in the next configuration/workflow pass

1. **Carry a cumulative repair count for the acceptance outcome.** Later checkpoints at lines 628 and 630–632 reported `repair_attempt: 0` despite the extended repair sequence. A local subproblem count cannot enforce an outcome-wide repair budget. Preserve both counts if needed, and identify which one the policy uses.
2. **Distinguish running work from an owner awaiting instructions.** Only genuinely progressing or externally blocked work warrants passive waiting. Retained ownership alone is insufficient.
3. **Prefer a suitable retained specialist for a narrow recheck.** Require a concrete context or independence reason to start another reviewer. Track the worker's identity and actual assignment, not just a generic role label.
4. **Keep fully specified mechanical requests direct.** A directory listing or file-format check should not need a fresh investigation worker. This should also limit unnecessary shadow observations and repeated reads of unchanged guidance.
5. **Validate changes against saved counterexamples before expanding authority.** Preserve cases where diagnosis, independent review, waiting, and fixture repair were necessary. Do not tune only to agreement with the lead. Keep the existing pilot restrictions; exercise it only when a normal, genuinely eligible question occurs under automatic routing, rather than manufacturing a fresh assignment to fill its quota.

No new JEV API call, historical replay, dispatch, or production-log entry was created for this assessment. Evidence and provisional labels are frozen separately in `/Users/crewroom/.omp/agent/artifacts/m63-jev-postrun-20261001/`: `manifest.json`, `saved-report.txt`, `selected-observations.json`, and `adjudication.json`, with copies of the source records. The global shadow-log SHA-256 is `f28432335ab39357e754d35ea4cd8a7afb3ae1c03e17bfdaaaa7d3aa71b4a911`. Current rubric/policy sources were saved too; their hashes do not prove they are byte-identical to the historically loaded implementation.

## Initial review outcomes (historical snapshot)

| Work | Outcome at the reviewed snapshot | What the evidence establishes |
| --- | --- | --- |
| M6.1 | Accepted; implementation revision `678de51` | 149 tests on rerun, build, and 12 E2E tests passed. Review found defects, and a repair introduced a recovery issue that required Astra diagnosis. Bounded real-process scenarios were exercised; this was not a full process sweep. |
| M6.2 | Accepted; implementation `34bb44c`, acceptance/handoff `1a49bbb` | 193 tests, build, and 16 E2E tests passed. Independent review found material defects; the first repair introduced replay and hold regressions. Provider checks used fixtures, not live Bitbucket delivery. |
| M6.3 Slice 1 | Accepted source/test candidate; no feature commit at that point | 196 tests, build, and 16 E2E tests passed for the recorded candidate. This evidence does not automatically cover subsequent Slice 2 edits. |
| M6.3 Slice 2 | Still open in the inspected journal | Conflict reconciliation and recovery work continued through additional repairs. Final full gates remained pending at the snapshot. PR-feedback continuation also remained unfinished. |
| the pilot repository PR 1151 | Merged at `e00ba35a` | Targeted tests, static checks, and browser evidence supported the candidate. Post-merge staging checks, full-day metrics, and rollout acceptance remained separate obligations. |
Private pilot details were removed during FOR-17 history sanitization. Original evidence is retained in a private backup. This placeholder is not executable validation evidence.

The latest M6.3 session spanned more than 13 hours of elapsed time and produced 16 worker transcript files. That is evidence of a long, fragmented execution cycle, not a measurement of continuous coding time or a precise cost estimate.

The primary acceptance record is [MVP progress](testing/mvp-progress.md). The status document also lagged the active journal: it still described asking before Slice 2 while Slice 2 work was already underway. Handoffs should reconcile this discrepancy at every accepted checkpoint.

Later evidence, through approximately 2026-10-01 04:51 UTC, showed the repair loop continuing. A real Git reproduction demonstrated that a child could survive controller termination and move HEAD after recovery had treated the operation as settled. The repair correctly retained an operator hold when termination could not be proved. Three new controller-crash regressions passed, but integrated acceptance was still pending at that snapshot. Finishing this round can close **Slice 2 only**; PR-feedback continuation remains a separate M6.3 slice.

## Initial JEV findings (original cutoff)

At the reviewed cutoff, the shadow log contained 617 records. The read-only pilot was enabled with a 20-dispatch limit, but its state recorded **0 of 20 executed dispatches**. Its 509 pilot events were all ineligible observations. There is therefore no pilot execution outcome from which to claim implementation quality or time savings.

For M6.3, 64 next-step observations were comparable: JEV agreed with the lead's declared action 27 times and differed 37 times. Another 21 observations were not comparable. These are agreement counts, not accuracy scores, and they do not establish which action was subsequently performed.

JEV sometimes identified a useful need for diagnosis, including resource-limit and interruption-recovery failures. It also recommended waiting when an approved next step was available, or continued reasoning after a diagnosis was available. Its choices were sensitive to how the coordinator described the problem: describing a defect as a small edit versus a required diagnosis changed the recommendation.

For M6.4:

- Keep JEV advisory. Do not make it a new dependency for completing a slice.
- Explain the pilot's zero eligible dispatches before expanding it.
- Evaluate recommendations against subsequent outcomes, rather than treating agreement with the lead as correctness.
- Include observation overhead in any future efficiency assessment; the current evidence does not demonstrate net savings.

The journal also shows repeated reads of the shadow guidance and checkpoint creation around status and retry messages. This is observable coordination overhead, although its share of total delay has not been measured. Read unchanged guidance once per context and record observations at meaningful decision boundaries; do not add another mandatory analysis step to every status reply.

## Proposed execution rules for the remaining M6.3 work and M6.4

### 1. Commit accepted work before starting the next slice

An accepted slice must have a commit before implementation begins on the next slice. Reference that commit in its acceptance evidence and handoff. During iteration, a complete source manifest remains useful; after acceptance, it should not be the only way to recover the tested candidate.

At recheck, HEAD was still M6.2 acceptance `1a49bbb`, with accepted Slice 1 and ongoing Slice 2 mixed in a large working diff. Establish a committed boundary before PR-feedback continuation. Do not reconstruct separate historical commits and claim that earlier tests covered them without verifying the source identities. If clean separation cannot be established, commit the combined accepted candidate after validation and document that boundary honestly.

### 2. Write the failure model before stateful implementation

Record a short failure model alongside the slice's contracts in `docs/architecture.md`, referring to existing contracts rather than restating them:

- **In scope:** the operations and interruption points the slice must handle, such as controller termination during each mutating Git operation; the required result may be a safe operator hold.
- **Required invariants:** ownership, preserved work, truthful receipts, evidence invalidation, and the conditions for another writer or continuation to start.
- **Known limitations:** capabilities the slice does not promise, such as automatic orphan termination or general supervision of hook descendants, provided the required invariants still hold.
- **Evidence:** a bounded acceptance case for each distinct failure class, with its expected recovery or hold behavior.

For cancellation, specify when execution has actually stopped and what later continuation resumes or creates. An unresolved contract should trigger one bounded investigation with a concrete decision as its output.

High-risk reviewers judge against this model. A newly discovered failure class receives explicit scope triage rather than automatically starting another repair worker. It can become a documented follow-up when existing promises still hold. A reproducible violation of an already accepted ownership or data-preservation contract remains a blocker even if the failure model omitted it; the model cannot turn an unsafe result into an accepted limitation.

### 3. Bound repair cycles across the whole outcome

Use these as proposed starting thresholds:

- After **60 active minutes without a reviewable increment**, checkpoint the work and identify the unresolved obstacle.
- After **two repair passes against the same acceptance outcome**, stop the repair loop and replan with a specific diagnosis.
- Carry the budget across worker replacements, renamed issues, and subdivisions of the same outcome.

These are checkpoints, not delivery promises or permission to waive quality gates. Record provider waits separately from implementation, testing, and repair time so the cause of delay remains visible.

Apply this rule to the remaining M6.3 work now, rather than waiting for M6.4. The reviewed Slice 2 sequence would already have exceeded the proposed limit; the original recommendation was not an enforced budget. Let an already running bounded check finish, then require an explicit scope and failure-model checkpoint before another repair cycle. Passing checks proceed to acceptance; another failure gets a named diagnosis and decision, not an automatic budget reset.

### 4. Keep ownership stable

Use one implementation owner and one retained independent reviewer per slice. The coordinator owns scope and acceptance. Keep small repairs with the existing worker; introduce a fresh worker when there is a new bounded outcome or a specific diagnosis requiring different expertise.

Request another general review only when it covers an explicit risk that the retained review does not address. Repeated handoffs should not substitute for a decision about the underlying invariant.

### 5. Validate a stable candidate

Run focused checks during iteration. Freeze the candidate before expensive acceptance checks, and identify the exact revision or source snapshot covered by review and test evidence. Run the required domain, build, and user-flow gates for the affected change: `npm test`, `npm run build`, and `npm run test:e2e`.

Reuse evidence only while its candidate and relevant assumptions remain valid; rerun checks invalidated by subsequent changes. A changed aggregate diff hash requires a new candidate identity, but it does not prove that every earlier check must be rerun. Determine which evidence the change affects. A repair budget expiring never turns an incomplete candidate into an accepted one.

### 6. Give provider failures their own handling rule

Use a bounded retry policy followed by an already authorized, validated fallback or an explicit checkpoint. Record the actual serving model and provider. Provider instability should not silently consume an unlimited implementation budget or obscure whether a model assignment was appropriate.

### 7. Close each remaining M6.3 slice before M6.4

After Slice 2 acceptance and its committed checkpoint, run PR-feedback continuation as a separate slice in a fresh coordinator session, under these same rules and the existing authorization boundary. Only then can M6.3 close. Start M6.4 from that accepted revision with a short handoff: accepted behavior, remaining obligations, exact evidence, and the first M6.4 slice. Confirm the routing mode before substantial work begins.

## Proposed M6.4 slices

These organize the existing [M6.4 scope](mvp-plan.md); they do not silently defer any of it. Include keyboard access and usable layouts in each affected slice.

| Slice | Acceptance outcome |
| --- | --- |
| A. Role and model editing | Edits persist; existing assignments retain their captured role/model definitions. |
| B. Repository editing | Repository changes validate correctly; queued work and historical references remain coherent. |
| C. Failure, usage, and history views | Users can inspect truthful failure explanations, usage provenance, and run history. |
| D. Steering feedback | Stored, received, and applied steering are distinguishable and accurately displayed. |
| E. Specialist insertion | Insertion occurs at a defined coordination point, creates one assignment, and handles affected evidence explicitly. |
| F. Cancel and later continuation | Cancellation ends execution; later continuation preserves work and history without accidental restart. |

Before implementing a slice, record its outcome, failure model, unresolved contracts, owner, validation plan, and repair budget. Inspect existing behavior first: specialist insertion and cancellation already have implementation and acceptance evidence, so these slices should finish the identified gaps rather than rebuild the mechanisms. Preserve the approved constraint against a separate direct-specialist steering workflow.

## Model-role review

### Configured roles versus actual execution

All 31 inspected OMP role aliases resolved to installed models with supported reasoning settings. Crewroom's own four configured roles also fit the intended division: Opus coordinator, Sol implementer, Opus reviewer, and Luna delivery reporter. These application roles are separate from the OMP agents developing Crewroom.

However, the reviewed M6.3 lead switched from Opus to Luna via the `default` role at 14:08 UTC on September 30, before the first user prompt at 14:10 UTC. That is an observed selection event, not proof that the user manually chose Luna. Routing was direct, classifier calls were zero, and three escalation attempts were refused because the user/session selection was authoritative. Worker assignments still used Sol, Astra, Opus, and Fable as expected, but **M6.3 was not a clean trial of the configured Opus coordinator workflow**.

At 22:14 UTC the user suggested switching to dynamic routing. The lead acknowledged the pin but did not explain how to change it. That was a communication failure as well as a routing mismatch.

When a user requests automatic routing and a pin prevents it, explain the blocker and the exact command: finish the current turn, enter `/route auto`, then submit the substantive continuation request. `/route auto` enables classification of the next request; it does not immediately switch the model. Confirm `/route status` reports worker execution with the intended coordinator before new substantial work. If changing the pin requires the user's UI action, yield with these instructions instead of continuing substantial work under the unwanted route. Preserve safe handling of already running work; do not abruptly terminate workers merely to change routing.

### Recommended assignments

| Responsibility | Model and reasoning | Assignment boundary |
| --- | --- | --- |
| Coordination, scope, acceptance | Opus 5.5, medium | Keep implementation detail with workers; maintain the acceptance and repair budget. |
| Requirements and source investigation | GPT-6 Sol, medium | Answer a bounded question with evidence and a decision. |
| Ordinary implementation and repairs | GPT-6 Sol, high | Own one slice through focused checks and review repairs. |
| Lifecycle, ownership, migration, or difficult recovery diagnosis | GPT-6 Astra, medium | Resolve the specific invariant before more implementation. |
| Cancel/continuation lifecycle implementation in Slice F | GPT-6 Astra, medium | Own the bounded state-machine change from diagnosis through tests. Use Sol for presentation-only gaps if existing lifecycle behavior already satisfies the contract. |
| Specialist-insertion ownership changes in Slice E | GPT-6 Astra, medium | Use when the slice changes lifecycle or ownership invariants; keep ordinary UI and feedback work with Sol high. |
| Independent review of OpenAI-authored code | Opus 5.5, high | Retain one reviewer across the slice. |
| High-risk concurrency or recovery review | Fable 5, high | Use for the identified risk; avoid automatically stacking routine reviewers. |
| Browser and integrated verification | GPT-6 Sol, high | Verify the frozen candidate against concrete acceptance cases. |
| Extraction, mechanical work, and established reporting | GPT-6 Luna, xhigh | Use bounded tasks with readily checkable outputs. |

For Anthropic-authored code, use Sol high for ordinary independent review or Astra medium for demanding review. Determine independence from the actual code producer, not the coordinator's model. Supplemental same-vendor review may be useful, but it does not establish cross-vendor review.

These are operational recommendations based on the observed work and available models, not benchmark-proven rankings. The broad distinction between focused work and demanding reasoning is consistent with [OpenAI's model guidance](https://developers.openai.com/api/docs/guides/latest-model); the coordinator recommendation also aligns with [Anthropic's Opus 5.5 guidance](https://platform.claude.com/docs/en/models/opus-5-5/overview).

Most rows already match configured aliases. The substantive changes are enforcing the effective coordinator selection, keeping difficult lifecycle implementation with one Astra owner, and validating reviewer provenance. One session does not isolate Sol's or Luna's contribution to the delay from missing contracts, handoffs, and provider failures.

### Availability and configuration gaps

- The live gateway catalog exposed 54 chat entries versus 50 installed entries at the snapshot. Newly exposed Azure paths for Sol and Luna were not yet installed or validated for this workflow.
- Existing Sol and Luna aliases used Bedrock paths. An alternate Azure path could be useful for provider fallback, but first validate tool use, reasoning settings, context limits, and actual serving identity on a bounded task.
- Published model availability differs from gateway availability. Newly published models absent from this gateway should not appear in the execution plan as available assignments.
- Other available model families lacked comparable Crewroom evidence. Adding them to critical roles during M6.4 would introduce another variable.
- The current default alias was Astra xhigh while the local routing guide still described Luna xhigh. Retain the current explicit setting unless the user chooses a different startup default; update the guide to describe it accurately. Automatic coordinator selection must be checked independently of this default. This review does not establish a price ranking among these settings.
- Opus, Sol, Astra, and Fable had empty fallback chains. Luna fell back to Sol xhigh; Sonnet fell back to Opus high. Availability and quota checks at one point in time do not establish provider reliability.
- A coordinator fallback from Opus to Fable is a reasonable candidate, subject to bounded validation and authorization. However, the inspected `retry.fallbackChains` configuration is keyed by model, not role. Adding an Opus fallback there would not be coordinator-only: it could affect other Opus assignments. Use an explicit coordinator reassignment at a checkpoint or a verified role-specific mechanism. Another Anthropic model also need not avoid the same provider outage. Keep implementation failures bounded at a checkpoint unless a fallback is deliberately selected.
- Some review dispatches lacked producer/vendor requirements. Require that metadata and verify the actual reviewer identity before crediting independent review.

For a fresh OMP session, select `/route auto` and confirm `/route status` after the substantial request is classified: worker execution with the intended Opus medium coordinator. `/route code` selects a direct coding route; it does not establish the coordinator workflow described here.

No model configuration, fallback chain, or routing implementation was changed by this review.

### Corrections to the adversarial response

Two proposed configuration corrections do not match the inspected sources:

- `review_high_risk_anthropic` names the **reviewer's** vendor, not the code producer's vendor. `omp-review/SKILL.md` explicitly selects it for OpenAI-authored work. Fable is therefore the correct current mapping; changing it to Astra would break the documented cross-vendor selection. Anthropic-authored demanding work already has `reviewer-deep`, which resolves `@request_deep` to Astra medium. The defect to address is dispatch metadata and effective selection, not this alias.
- The response describes Fable→Opus fallback, but the current configuration has **Sonnet→Opus** and an empty Fable chain. Its proposed coordinator-only fallback also needs the role-scoping qualification above.

The stronger commit rule, failure-model requirement, immediate repair checkpoint, explicit M6.3 remainder, and routing instructions are incorporated. Astra ownership is strengthened for actual lifecycle changes, with presentation-only work remaining on Sol. Known limitations remain subject to the already accepted safety contracts.

## Suggested M6.4 kickoff instruction

> Complete one named slice at a time, including the remaining M6.3 work. Confirm automatic worker routing and the actual Opus medium coordinator with `/route status` before substantial work. Record the outcome, failure model, unresolved contracts, owner, validation, and repair budget before editing. Count repairs across worker replacements. If a new architectural prerequisite appears or the budget is exhausted, checkpoint and replan before expanding implementation. Report accepted criteria, remaining blockers, and elapsed time by category. Preserve required quality gates, commit accepted work before the next slice begins, and stop at the agreed slice boundary. Completing conflict reconciliation does not complete PR-feedback continuation or M6.3.

## Evidence references

- Repository: [status](status.md), [MVP plan](mvp-plan.md), [executed evidence](testing/mvp-progress.md), [product direction](product.md), and [architecture](architecture.md).
- M6.3 primary session: `/Users/crewroom/.omp/agent/sessions/-Projects-Git-crewroom/2026-09-30T13-53-38-556Z_01a0f297-a0fc-73ab-9775-285f7ba99b61.jsonl` and its worker transcript directory.
- M6.1 and M6.2 sessions: the Crewroom journals beginning `2026-09-30T01-57-01-452Z` and `2026-09-30T04-10-33-582Z` in the same session root.
- JEV: `/Users/crewroom/.omp/agent/state/jev-shadow/events.jsonl` and the `readonly-pilot` state and event records. Shadow-log cutoff: 2026-10-01 03:33:44 UTC; SHA-256 `f52a0c0cbb3eb303a4fdd74070dac5d3fb5110e1e7c770861b7ab4d2aba10b88`.
- Routing: `/Users/crewroom/.omp/agent/config.yml` and local `model-routing.md`, `worker-workflow.md`, and `selective-workflow.md` under `/Users/crewroom/.omp/agent/docs/`.
- Routing controls: `/Users/crewroom/.omp/agent/extensions/request-routing.ts`, especially the command handler and escalation guard. Review selection: `/Users/crewroom/.omp/agent/skills/omp-review/SKILL.md`, `/Users/crewroom/.omp/agent/docs/review-policy.md`, and `/Users/crewroom/.omp/agent/agents/reviewer-deep.md`.
- Crewroom application roles: `/Users/crewroom/.config/crewroom/roles.json`.
- M6.3 Slice 1 evidence identity: base HEAD `1a49bbbf783caa70ebfd407e839a91a9ae41e8ac` plus source/test diff SHA-256 `5a9dffddd01c5f4f4063ca8419cf8c6d0d7b56ca8c5f607f1a96bcfcd24b2f43`.

The personal runtime paths above are local evidence pointers, not portable repository dependencies. The review did not rerun application tests, execute pilot dispatches, or establish new live-delivery evidence.
