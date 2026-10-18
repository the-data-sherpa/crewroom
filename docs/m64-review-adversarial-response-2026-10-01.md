# Adversarial response to the M6.4 execution and model-role review

Date: 2026-10-01
Reviewer: Claude (Opus 5.5), for Christopher
Responds to: [m64-execution-and-model-review-2026-10-01.md](m64-execution-and-model-review-2026-10-01.md)

Evidence inspected: the review itself; the live M6.3 primary journal (`2026-09-30T13-53-38-556Z_01a0f297…jsonl`, through 2026-10-01 ~04:51 UTC) and its 16 worker transcripts; `git log`/`git status` in this repo; `~/.omp/agent/config.yml`. No tests were rerun and no configuration was changed.

## Verdict

The review's main finding holds: most M6.3 delay came from discovering lifecycle and ownership requirements mid-repair, not from model choice. Its evidence discipline is good (JEV agreement is not treated as accuracy; the 0/20 pilot dispatch count is stated plainly). The challenges below are about emphasis and omissions, plus one config contradiction.

## Challenges to the execution findings

### 1. Commit discipline is the first rule, not the fourth

HEAD is still `1a49bbb` (M6.2 acceptance). M6.3 Slice 1 was accepted many hours ago and is still uncommitted, mixed with Slice 2 in one dirty diff: 36 files, ~4,650 inserted lines. Every evidence identity in the session is a hash of that dirty diff, so every repair invalidates it and reviewers must "isolate only Slice 2 changes" by hand. The review lists "commit accepted increments" inside recommendation 4; it should be an explicit precondition for starting the next slice.

Proposed rule: an accepted slice is committed before any work on the next slice begins. Evidence identities reference commits, not dirty-diff hashes, wherever possible.

### 2. The missing artifact is a failure model, not just a lifecycle contract

Recommendation 1 ("resolve stateful contracts before coding") is right but too general. The specific gap that kept Slice 2 open is that nobody wrote down *which failures are in scope*. Without that, a high-risk reviewer examining recovery code will always find another crash window, and each one is individually defensible.

The latest example (~04:21 UTC): a real Git reproduction with a blocking `pre-rebase` hook showed a Git child surviving SIGKILL of its Node `execFile` controller, `rebase --abort` no-op'ing for lack of metadata, and HEAD moving after recovery had settled. That is a real defect, and the fail-closed hold chosen is the right cheap fix. But it was discovered after ~9 consecutive repair workers, and nothing prevents the next one.

Proposed rule: each stateful slice records a short failure model in `docs/architecture.md` before implementation:

- In scope (must be handled, typically fail-closed to an operator hold): e.g. controller crash during any mutating Git operation.
- Explicit known limitations: e.g. orphaned descendants of hooks, automatic orphan termination, generic process supervision.

High-risk reviewers judge the candidate against that model. A new failure class outside it is logged as a limitation or follow-up, not an acceptance blocker, unless the coordinator explicitly reopens the model.

### 3. The repair budget would have tripped hours earlier — and the loop continued after the review

Slice 2 implementation/repair workers since 2026-09-30 20:51 UTC, all Sol unless noted:
`M63S2RetryRepair`, `M63S2PostAdoptionDrift`, `M63S2SnapshotIdentity`, `M63S2PromotionRecovery`, `M63S2CheckedProcessLease`, `M63S2BoundedGitOutput`, `M63S2PromotionLeaseIntegration`, `M63S2RetryHoldAndUX`, plus long-running Astra diagnosis (`M63S2RetryReasoning`) and Fable review (`M63S2FinalRiskReview`).

The review's "two repair passes against the same acceptance outcome, then replan" rule would have fired well before this. After the review's 03:33 cutoff, a new finding (ordinary `execFile` rebase window, ~03:57) widened scope again at ~04:21. The review should state that the budget applies *now*, not only from M6.4.

### 4. "Wrap this up" does not close M6.3

When the current round finishes and passes recheck and integrated gates, Slice 2 (conflict reconciliation) can close. **PR-feedback continuation is still open.** The handoff should not describe M6.3 as complete. Recommendation: commit Slice 1 and Slice 2, then run PR-feedback continuation as its own slice in a fresh session under the M6.4 contract (outcome, failure model, owner, validation, repair budget).

### 5. The routing failure was also a communication failure

The lead switched from Opus high to Luna xhigh ~15 minutes into the session (14:08 UTC), before the first user prompt; it is worth confirming whether that was manual or a default/alias effect. At 22:14 the user asked to "switch to the dynamic route." The lead reported that the pin was authoritative and continued on Luna without telling the user how to unpin. The review describes this as "surface a routing mismatch"; it should be a hard rule: **when a pin blocks a routing change the user requested, the lead tells the user the exact command to change it and stops until they choose.**

### 6. JEV overhead is visible per turn

The lead re-reads `jev-workflow-shadow.md` and records a `next_step` observation on essentially every user message, including "what's the holdup?" and "wrap this up." That supports the review's point that observation overhead must be counted before claiming savings.

## Challenges to the model-role review

### The table is mostly the current config

Nearly every recommended row already matches an alias in `config.yml` (`request_coordinator` Opus medium, `code` Sol high, `diagnose`/`architect` Astra medium, `review_high_risk` Fable high, Luna xhigh for mechanical work). The substantive change is ensuring the coordinator actually runs as Opus. The review should say so plainly so it is not read as a large reassignment.

### Agree

- **Opus coordinator.** Luna was diligent about protocol (hashes, ownership, pauses) but did not exercise the judgment the coordinator seat needs: it never halted the repair loop and accepted each scope expansion as necessary. Causation can't be proven from one run, but Luna xhigh is undersized for that seat.
- **Fable for scoped high-risk review.** It found the real defects. Note it initially judged the orphaned-rebase issue non-blocking and revised only after the lead's reproduction — another argument for giving it an explicit failure model to judge against.
- **No new model families in M6.4.**

### Change

1. **Astra as default implementer for Slice F (cancel and later continuation), likely Slice E (specialist insertion).** The review makes Astra implementation an exception. M6.3 shows lifecycle/recovery work is exactly where Sol churned through repeated repairs. Part of that was the missing contract, but F is the same class of state-machine work; keep diagnosis-through-tests with one Astra owner to avoid a lossy handoff.
2. **Fix a cross-vendor contradiction.** The review says Anthropic-authored code should get Sol/Astra review, but `review_high_risk_anthropic` is `anthropic.claude-fable-5:high` (same vendor). Point it at Astra medium (`judge_alt` already is).
3. **Give the coordinator a fallback.** Opus, Sol and Astra have empty fallback chains; only Luna→Sol xhigh and Fable→Opus high exist. An Opus provider failure as lead either stalls or ends up re-pinned to a weaker model. Add Opus→Fable for the coordinator role only. Leave implementer chains empty deliberately and have provider failure stop at a checkpoint (consistent with recommendation 5).
4. **Resolve the `default` alias.** It is `gpt-6-astra:xhigh` (the most expensive setting in the file) while `model-routing.md` says Luna xhigh. Decide which is intended and make the doc and config agree.

## Requested changes to the review document

1. Promote "commit accepted increments" to a precondition for the next slice; reference commits in evidence identity.
2. Replace the general "contracts before coding" language for stateful slices with a required failure model (in scope / known limitations), and instruct high-risk reviewers to judge against it.
3. State that the repair budget applies to the remaining M6.3 work, and record that the loop continued past the review cutoff.
4. Correct any implication that finishing the current round closes M6.3; list PR-feedback continuation as the next slice.
5. Make "tell the user how to unpin when a pin blocks their routing request" a hard rule.
6. Note that the model table largely restates current config; add the four changes above.
7. Add to the kickoff instruction: "Confirm `/route status` shows an Opus coordinator before substantial work; write the slice's failure model before editing."
