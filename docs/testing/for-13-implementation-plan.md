# FOR-13 — Corrections become enforceable checks: implementation plan

**Implementation completed:** see the [acceptance receipt](for-13-corrections-2026-10-13.json), [correction ledger](corrections/README.md) and [executed progress](mvp-progress.md#for-13--s92-implementation-and-validation-2026-10-13). The dated planning text below retains its original pre-implementation status.

Planning checkpoint: **2026-10-13**, inspected `main@109b430f2b70d516fe28e66e4a59b0ed4a57c9cb`. The operator requested this plan after FOR-12 acceptance. [FOR-13](https://linear.app/forgeinnovations/issue/FOR-13/s92-corrections-become-enforceable-checks) remains Backlog: implementation, fresh historical regression proof and the mutation trial have not started. FOR-12 is Done; FOR-14 follows this item.

This plan implements [S9.2](../workbench-redesign.md#s92--corrections-become-enforceable-checks) within the accepted [product](../product.md), [architecture](../architecture.md) and [reuse assessment](../research/omp-team-reuse-assessment.md). [Status](../status.md) and [progress](mvp-progress.md) distinguish planning from executed acceptance. The [review page](../../.lavish/for-13-implementation-plan.html) presents these proposed choices.

## Outcome and scope

Turn one genuinely recurring correction into a traceable claim: two distinct recorded occurrences → justified enforcement layer → an existing check that rejects the historical defect and accepts legitimate behavior. Separately assess the strength of tests for one small evidence module through five explicit mutations. A valid outcome may be “existing enforcement is sufficient” or “the trial found no useful gap.”

**Recommended implementation:** a repository-local correction ledger, a read-only validator, one portable historical regression reproducer and a bounded mutation runner. Reuse existing tests and S7's operator-reviewed learning path. This matches the acceptance criteria without adding a database table, native screen or another workflow authority.

The ledger records facts and links. Validation never queues work, writes roles, changes an admitted assignment, marks a coding candidate ready or substitutes for independent review. A future product editing surface is separate work if actual usage establishes a need.

**Excluded:** automatic mining/clustering of every conversation; a general mutation framework or global score threshold; new roles or runner tools; automatic proposal/adoption; app schema migration; repeated MH6 live jobs; FOR-14 instruction comparisons; paid model calls; live Jira/PR delivery; whole-state restore; platform/package/terminal certification. No production fix is presumed necessary.

## Inspected reuse and selected example

S7 already validates same-run learning citations, records keyed proposal replay and operator decisions, rejects stale base versions, reconciles approval before role-file writes and retains role/guidance provenance. Admitted assignments keep captured instructions. Relevant modules are [learning](../../crates/crewroom-core/src/learning.rs), [role history](../../crates/crewroom-core/src/role_history.rs), [roles](../../crates/crewroom-core/src/roles.rs), [attempts](../../crates/crewroom-core/src/attempts.rs) and [teams](../../crates/crewroom-core/src/teams.rs). Native [Learning](../../crates/crewroom-desktop/src/workbench/collaboration/learning.rs) and [Settings learning/history](../../crates/crewroom-desktop/src/workbench/settings/learning.rs) already expose review and decisions.

That mechanism is not a recurrence classifier. Coordinator proposals accept existing same-run check/review/steering/check-policy IDs; operator role proposals cite an operator note. Historical repository review files are not valid product evidence IDs. Do not manufacture a run or forge a citation to fit them into S7. Schema remains 29 under this proposal.

### One recurring class: mutable validation before committed replay

**Invariant:** within the authenticated request path, check immutable request identity and return an already committed receipt before revalidating mutable state that belongs only to a new request. A different payload under the same key must still conflict. This is not permission to bypass caller authentication or run/attempt scope.

| Distinct observed occurrence | Available original reference | Existing enforcement and limits |
| --- | --- | --- |
| R8 learning-proposal replay consulted the changed target before its saved call | The independently reviewed P2 is recorded in `docs/testing/mvp-progress.md` at commit `062e30b70fd9f50cb6eb02c99bc26b2239e7a529`; original ledger blob `bff247c2e8dad3a57b7d0eca441de30351768dab`, R8 replay paragraph. Current ledger location is near line 1044. | [Learning.propose](../../server/learning.ts) and the existing [R8 replay test](../../tests/learning.test.ts). The original reviewer transcript and pre-fix executable were not found during planning. The checked-in historical review record is the available occurrence evidence; do not claim stronger provenance. |
| MH6 package-report replay reread changed files before its saved report | Finding 3 in `/tmp/for11-spec-review.md`, reviewed against `2a9bf0c0f6c7f5ae24426e214974208527f5d7df`; the [MH6 source-bound receipt](mh6-pair-2026-10-13.json) identifies the retained review. An original diagnostic test and log remain locally available. | Fixed by `289b4e7b2246c65540214561f01823b627a1dab5`: early `Store::replay_package` in [choreography handling](../../crates/crewroom-runtime/src/supervisor/workflow/choreography.rs), backed by the [core receipt ledger](../../crates/crewroom-core/src/choreography/records.rs). [Existing daemon regression](../../scripts/rust/choreography.test.ts) proves saved replay after a real later edit and changed-payload refusal. |

These are two independent review corrections. A diagnostic fixture, a later summary of the same finding and a rerun are not extra occurrences. Before implementation acceptance, independently resolve both sources and confirm the common cause; if either cannot be substantiated, leave the recurrence claim incomplete. Do not relabel a one-off or invent another incident.

The strongest practical remedy is **domain/interface ordering with behavioral regression**. Types alone cannot know whether a particular request committed or whether current file/role state differs from that receipt. A generic static rule would not establish the correct ordering for every lifecycle. Guidance may explain the invariant, but the existing execution path and tests enforce it. Reuse them; no new production defect or duplicate enforcement ticket is needed.

### Preserve the MH6 diagnostic before relying on it

The three private files existed and were hashed during source inspection:

| Artifact | SHA-256 |
| --- | --- |
| `/tmp/for11-spec-review.md` | `bdc57b0a73159784071a877298d1b5db6ac4b3b574fe5917876f75f4f8e45869` |
| `/tmp/for11-review-replay.test.ts` | `9b9cd49105ebbaf0c52732eb3b9394c7bedf799f323912cf17553fff67630cbd` |
| `/tmp/for11-review-replay.log` | `9d729710496a3a127a9eb112ed2d712df8744d1b9dc580fd204af61d09f4cef1` |

At execution start, recheck hashes and copy attributable private evidence into a retained, owner-only FOR-13 directory. Commit only necessary sanitized excerpts with exact extraction provenance, the original artifact hash and the original occurrence identity. A rewritten summary is not a recovered original. Retain unavailable-source limitations rather than silently replacing missing evidence. The historical bad/repaired choreography Git blobs are `1d5bcc17d4bb826ffb4f8849b5b69bdb90e96855` and `64123b4eef0fddb6d79e3e3a7a3149242fe48569`.

## Record contract and small interface

Use one JSON record for the selected class and Git for its version history. Do not add a general registry, database or workflow state machine. The validator owns reference resolution, duplicate handling, proof-shape checks and honest result classification behind a single interface.

| Field group | Required content and rule |
| --- | --- |
| Identity | `schemaVersion`, stable class ID, FOR-13, repository identity, invariant and bounded scope. Reject unknown fields/versions and duplicate class IDs. |
| Occurrences | Original incident/event identity, observation time or explicit unknown, affected revision, observed bad behavior, source references and why this is independent of other occurrences. At least two resolved original identities are needed for the recurrence claim. |
| References | A fixed Git commit/path plus blob content SHA-256 and JSON pointer or bounded excerpt; or a private retained relative path/hash beneath an explicitly supplied evidence root. Never arbitrary commands, network fetches or an unqualified URL. Resolve symlinks/containment and reject traversal or ambiguous extraction. |
| Remedy | `domain_api`, `types`, `static_analysis`, `behavioral_test` or `guidance`; primary layer, supporting checks, selection rationale, stronger layers considered and existing enforcement inspection. The chosen class uses `domain_api` with behavioral proof. |
| Enforcement mapping | Exact source revision/path/symbol, existing test or portable reproducer, historical bad-source identity and legitimate input behavior. Commands are descriptive evidence, not executable instructions read from the record. |
| Verification | Bound source/binary/test identities; selected/executed counts; expected bad-case failure and legitimate success; retained outputs and cleanup; independent review and explicit limits. An exit-zero filename is insufficient. |
| Disposition | Existing enforcement reused, structural work proposed, or judgment/guidance proposed. Optional genuine task/proposal/decision/revision links retain their actual status; a link alone proves neither admission nor adoption. |

Keep facts separate: reference resolution, recurrence assessment, enforcement proof and proposal disposition. Missing evidence can coexist with an existing test; a mechanically valid record does not prove its semantic claims. The validator derives the number of distinct resolvable occurrence identities. An independent reviewer must still confirm that two records describe separate events with the same cause, and that the selected check establishes the claimed behavior. Bind that review to `claimSha256`: SHA-256 of the UTF-8, sorted-key, compact JSON record with only the top-level independent-review reference and generated validation-output references excluded; preserve array order and reject duplicate keys/nonfinite numbers. The review also names the resolved occurrence/proof hashes. Changing the invariant, occurrences, remedy, disposition or proof makes the review stale until renewed. Excluding only those reference fields avoids a circular hash; a bare `reviewed: true` never supplies review evidence.

Proposed commands, **not implemented by this planning change**:

```sh
python3 scripts/verify-corrections.py check docs/testing/corrections \
  --evidence-root .crewroom/test-results/for13/evidence \
  --output .crewroom/test-results/for13/validation-01.json

python3 scripts/mutation-trial.py artifacts --evidence-dir .crewroom/test-results/for13/mutation-01
```

`check` reads sources and writes only a new requested validation receipt. Existing output is refused, not overwritten. Use outcome `verified` only when the claimed record's required reference/proof fields resolve; retain separate `recurrenceAssessment` and `enforcementProof` fields and their review status. `incomplete` covers missing evidence, unresolved review and absent proof; `invalid` covers malformed content, source mismatch and unsupported claims. Exit codes should distinguish those outcomes. A one-off record is valid as a one-off and cannot satisfy the FOR-13 recurring-class gate. Guidance may be valid as a proposal while never receiving a mechanical-enforcement claim.

### Guidance and structural follow-up

For the selected already-enforced class, record `existing_enforcement_reused`; create no ceremonial guidance proposal or structural ticket. For judgment-dependent advice, record “recommended, not submitted” until a genuine S7 proposal exists. Role guidance may use native Ask/Teach with an operator note. Arbitrary cross-run repository evidence import into coordinator project-guidance proposals is outside this slice.

If an actual proposal is made later, link its true target/base version, proposal ID, operator decision and resulting role/guidance revision. Approval remains S7's action; completed validation never adopts it or changes an admitted assignment. Record fixture evidence as fixture evidence, never as an operator decision.

Any production defect found by the trial becomes bounded CrewRoom work with acceptance criteria unless explicitly included in FOR-13's implementation scope. Task creation records proposed work, not execution. Preserve the existing queue/admission, independent review and human merge rules. No automatic queueing follows classification.

## Historical bad-case and legitimate-behavior proof

Preflight the documented repository environment once: Python ≥3.12, Node ≥24, lockfile-installed Node dependencies, Git, Rust/locked dependencies and the existing native bootstrap/toolchain. Select the installed Node 26 path documented in the [feature index](feature-map/README.md) on this machine. Disposable checkouts use their own build outputs and fixture state; use the historical revision’s supported build requirements for the old proof. No credential refresh, live provider, Docker image pull or active-installation mutation is needed. Missing prerequisites leave the dependent lane unavailable.

The retained MH6 diagnostic originally **passes by observing the defect**: save a report, make a legitimate later file edit, then confirm same-key replay wrongly throws “Package artifact changed.” Its exit zero must not be reported as a regression check passing correct behavior.

Create a portable regression fixture derived from that diagnostic, retaining its source hash and diff. Replace absolute imports with repository-relative imports, and invert only the final bad expectation: same-key/same-input replay must return the saved receipt after the edit. Also retain the existing changed-payload refusal. Exercise the same fixture in disposable old and current checkouts:

1. Build the required historical fixtures at `2a9bf0c…`; the correct-behavior assertion must fail specifically at saved replay, after setup and earlier assertions pass.
2. Build current fixtures at the implementation revision; unchanged replay must equal the stored receipt, changed payload must conflict, and the original legitimate report/edit path must pass without duplicate records or additional writes.
3. Record full source/binary/fixture identities, exact failure assertion, selected test count and cleanup for both. A setup error, zero tests, import failure, timeout or unrelated failing assertion is not historical-defect detection.

The current broad [choreography test](../../scripts/rust/choreography.test.ts) contains a newer pending-exchange recovery segment. Do not run it unchanged on the old revision and count an earlier recovery failure as replay detection. The minimal derived fixture localizes the historical cause; the existing modern test remains the current regression gate.

If the old checkout cannot build after one classified environment repair, stop that lane. A causally isolated transplant of the exact historical missing-replay behavior into a current disposable checkout is a possible revised method, but must be labelled **historical-source transplant**, retain its diff and receive independent review. It cannot be described as running the original historical binary. No production source changes are needed merely to demonstrate existing enforcement.

## One-module mutation trial

Target exactly [crates/crewroom-runtime/src/evidence/artifacts.rs](../../crates/crewroom-runtime/src/evidence/artifacts.rs): 32 lines at the planning source, SHA-256 `283149d06fbcd73fb06f852892013d836df246c0fc8909735f7c9ef5bbebc763`. It checks artifact creation/read size, recorded byte count and content hash, delegating UUID and regular-file checks to existing helpers. This evidence module is independent of the replay correction example; S9.2 does not require both to be the same module.

Reuse the exact existing test in [scripts/rust/evidence.test.ts](../../scripts/rust/evidence.test.ts):

```sh
cargo build -p crewroom-runtime --bin crewroom-process --example repository-fixture --locked
node --import tsx --test --test-reporter=tap --test-concurrency=1 \
  --test-name-pattern '^native artifacts enforce byte hashes, identities, size and regular private files$' \
  scripts/rust/evidence.test.ts
```

Baseline setup builds both binaries: `repository-fixture` resolves `target/debug/crewroom-process` at startup even for this selected artifact test. Fingerprint both; per-mutant builds can select `--example repository-fixture` because only the artifact module is patched.

It currently accepts a Unicode roundtrip with correct byte length and rejects wrong hash, malformed ID, oversized writes and symlink substitution. Selected alone, it does not execute its neighboring Docker/provider cases. Verify exact test selection; none of these planning statements is a fresh test result.

Use a small versioned runner over **five reviewed, exact source patches**, not a generated workspace-wide campaign. The tool identity is its Git revision/content hash; config fixes the source preimage, patch identities, commands, test filter and budgets. This composes the existing external Node fixture test without introducing a separate Rust test that merely duplicates it.

| ID | Exact intended change within this module | What to inspect |
| --- | --- | --- |
| M1 | Omit the read-time content-hash comparison; retain byte-count validation. | Does a same-length hash mismatch fail the existing assertion? The current test changes the expected hash, not the file contents. |
| M2 | Omit the read-time recorded-byte-count comparison; retain content hash. | Does the suite exercise a false byte count with otherwise valid content? |
| M3 | Change the read validation's `||` to `&&`. | Are the two independent failure predicates actually distinguished? |
| M4 | Disable the write-time oversize guard. | Does the oversized-write test reject the weakened implementation? |
| M5 | Change the write limit from `>` to `>=`. | Does legitimate exactly-at-limit data remain accepted? |

These are proposed mutants, not observed survivors or known product defects. Freeze their literal old/new text and hashes after confirming the target still matches. Apply exactly one patch at a time to a disposable verification checkout. Refuse an unexpected preimage, multi-match replacement or any changed path outside the selected file. Restore the exact baseline between trials and verify it by hash; never patch the main checkout.

The [native repository adapter](../../scripts/rust/repository-adapter.ts) hardcodes `target/debug/examples/repository-fixture` relative to its working directory. Run the Node test from the disposable checkout and rebuild/fingerprint **that checkout's binary for every mutant**. Do not share main's target executable or assume a Cargo cache proves rebinding. Lockfile-identical dependency reuse is acceptable; record it. No adapter or production executable-path change is needed.

Budgets are proposed stop limits: one worker; five mutants; baseline setup/build at most 10 minutes; each subsequent build at most 3 minutes; each selected test at most 45 seconds; trial total at most 30 minutes including baseline, builds and tests. Log measured durations. A limit stops and records unfinished work rather than extending itself. Capture the original baseline before mutation and after any assertion improvement; retain all failed attempts. At most two useful assertion improvements belong in this slice; wider changes become bounded follow-up work.

Report each mutant as `killed`, `survived`, `unviable`, `timeout`, `infrastructure_error` or `not_run`, with the actual assertion/compile result and runtime. Only a failure attributable to the intended behavior is a kill. Timeout and compilation failure are not behavioral detection. For every survivor, independently classify an actual behavioral gap, equivalent change, irrelevant behavior or unresolved result, with a reason. Do not call a survivor equivalent merely because the test passes.

Any added assertion must pass the original legitimate behavior and reject its relevant mutant; record pre/post test hashes and outcomes. Retain the full five-mutant matrix even when some patches are unviable. Zero selected tests, zero exercised viable mutants, unclassified survivors or unfinished required runs cannot close the trial. There is no target mutation score, complexity threshold or requirement to find a gap.

The helper must retain a new private output directory, exact checkout/run identities, commands, exits, stdout/stderr, binary/source/patch hashes, selected test counts and timing. The current evidence fixture deletes its temporary state during teardown; retain the test output and trial checkout before removal, and add only the narrow failure-retention/exit check needed if diagnostics or owned cleanup cannot otherwise be proved. Use existing [owned process tooling](../../scripts/test_processes.py); verify native fixture exit and stop only trial-owned children. An unresolved child makes the attempt incomplete. Preserve the evidence outside disposable worktrees before removing only those worktrees.

## Proposed files and work sequence

| Location | Responsibility |
| --- | --- |
| `docs/testing/corrections/README.md` and `replay-before-revalidation.json` | Compact contract, one class, authentic occurrence provenance, remedy/proof/disposition mapping and limits. |
| `scripts/verify-corrections.py`, `scripts/test_verify_corrections.py` | Single read-only claim-validation interface and focused CLI guards; no command execution from records. |
| `scripts/rust/correction-replay.test.ts` | Portable minimal derivation of the retained MH6 reproducer; provenance and expected old/new behavior. Name is proposed, not an existing test. |
| `scripts/mutation-trial.py`, `scripts/test_mutation_trial.py`, `docs/testing/mutations/artifacts/` | Fixed five-patch configuration, isolated build/test/retention orchestration and guard tests. No framework registry or automatic expansion. |
| Existing `scripts/rust/evidence.test.ts` | Reuse selected test; improve only demonstrated missing assertions and any necessary owned-exit/retention evidence. |
| `docs/testing/for-13-corrections-<date>.json`, status/progress | Sanitized acceptance receipt, historical failures, mutation costs and truthful handoff. |

1. **Resolve sources.** Refresh Linear; confirm FOR-12 Done. Recheck and retain the two occurrence sources, historical diagnostic and exact source revisions. Independent review must agree they are distinct and substantiate the same cause before calling the class recurring.
2. **Write the record contract.** Create the one correction record initially marked unverified. Implement reference validation and non-claims for missing, one-off, duplicate or judgment-dependent evidence.
3. **Prove existing enforcement.** Run the minimal historical/current replay fixture and current regression. Record the exact causal failure on bad source and legitimate/changed-payload behavior on current source. Reuse the production check unchanged unless a real defect is discovered.
4. **Run the fixed mutation assessment.** Preflight dependencies once, build the baseline, execute five isolated patches under the fixed budget, classify survivors and make only useful bounded assertion improvements.
5. **Check authority preservation.** Verify validator/runner do not mutate configuration, workflow database, queue or main source. Guidance remains proposed unless linked to a real S7 decision; structural work remains outside execution without admission. No fake product proposal is required.
6. **Review and close.** Independently trace each acceptance item to revision-bound evidence; run applicable changed-code checks; record limits and exact commits; update and fetch Linear plus status/progress. Only completed acceptance moves FOR-13 to Done. FOR-14 remains separate.

## Bounded testing and stop rules

For **this planning change**, check local references, historical source/hash availability, consistency with Linear/accepted contracts, and review-page rendering/interactions. Do not run application acceptance, reproduce the old defect or execute mutations merely to prepare the plan.

| Implementation change or claim | Required verification |
| --- | --- |
| Correction validator | Public CLI guards: valid distinct references; duplicate same-incident references; one-off; absent/hash-mismatched source; bad extraction; malformed/unsupported schema; missing bad/good proof; guidance falsely marked enforced/adopted; structural link falsely marked executed; refused path traversal/output overwrite. Test that it leaves user/app state unchanged. |
| Historical enforcement | Same minimal fixture on identifiable historical/current source; exact causal failing assertion vs legitimate success; changed-payload rejection and no duplicated durable effect. Existing R8 replay/current choreography tests supply relevant supplementary coverage, not new occurrences. |
| Mutation runner | Preimage/path restriction, clean restore, per-mutant rebuild/binary binding, exact nonzero test selection, timeout classification, retained failure output and owned cleanup. Use disposable tool fixtures for helper guards; those are not mutation results. |
| Mutation acceptance | Unmodified baseline plus all five recorded outcomes; independent survivor classification; original/relevant-mutant pair for each improved assertion; final source fingerprints and runtime cost. |
| Python-only helper/docs work | Focused helper tests, Python syntax, references/JSON/hash checks and the actual planned proof/trial. No full application suite just for prose. |
| Type/build or native fixture changes | `npm run build`; explicit affected fixture builds and selected runtime tests. Repository tsc excludes `scripts/rust/**/*.ts`, so runtime execution is required for those files. |
| Production domain changes, only if explicitly included | `npm test`, affected Rust core/runtime/service tests, build/format and independent review. Reassess scope before enlarging the remedy. |
| Actual user-flow or S7 UI changes | `npm run test:e2e` plus affected native GPUI journey. Proposal/decision, one provenance-linked revision, unchanged admitted assignment and replay/reopen must be observed on the actual surface. No such changes are proposed here. |
| Steering, pause/resume or readiness behavior changes | Re-execute the affected [FOR-12 feature recipe](feature-map/README.md) at the final applicable revision. Unchanged recipes are not rerun for ledger-only changes. |

If S7 code is unexpectedly touched, select existing [core learning](../../crates/crewroom-core/tests/learning.rs), [role learning](../../crates/crewroom-core/tests/role_learning.rs), service `learning_decisions_and_startup_reconciliation_keep_one_catalog_revision` and desktop `operator_role_proposal_history_and_manual_drafting_fallback` checks as applicable. Keeping S7 unchanged allows its existing evidence to remain historical rather than repeating the full learning oracle.

Preserve failures and classify before another attempt: evidence unavailable, helper/setup defect, product regression, mutation result or uncertain ownership. After two unsuccessful reruns of the same classified failure, stop that lane and record its next bounded action; additional runs require new evidence or a revised testing decision. Never count a repaired document, a skipped test, a quiet terminal, an unavailable historical source or a timed-out mutant as acceptance.

The final receipt must distinguish inspected history from fresh execution, actual defects from proposed mutants, record validity from semantic review, scripted fixtures from live delivery, and existing enforcement from newly implemented changes. Large/private raw artifacts remain retained by path/hash; sanitized committed extracts remain traceable to their originals. Commit and push tested work per repository instructions; verify the exact remote commit and Linear project/state/content before reporting completion.
