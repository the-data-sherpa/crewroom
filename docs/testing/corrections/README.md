# Correction evidence

[FOR-13](https://linear.app/forgeinnovations/issue/FOR-13/s92-corrections-become-enforceable-checks) records one recurring correction, its existing enforcement and a fresh historical regression. [The record](replay-before-revalidation.json) concerns committed replay before mutable revalidation. The [historical proof](../for13-historical-replay-2026-10-13.json) runs the same portable fixture against original bad and repaired source. The separate mutation trial evaluates test strength; mutants are not additional correction occurrences.

Run the read-only validator from the checkout:

```sh
python3 scripts/verify-corrections.py check docs/testing/corrections \
  --evidence-root .crewroom/test-results/for13/evidence \
  --output .crewroom/test-results/for13/validation-01.json
python3 -m unittest scripts/test_verify_corrections.py
```

The output parent must already exist. The CLI creates only the requested new receipt, with mode `0600`; an existing file or output symlink is refused. Omit `--output` to print the receipt without writing it. It runs fixed read-only Git plumbing to resolve pinned blobs. It never executes a command supplied by a record, contacts a provider, queues work, writes a role, approves a proposal or changes an assignment.

| Outcome / exit | Meaning |
| --- | --- |
| `verified` / `0` | Required references, proof shape and independent-review binding resolve for the stated claim. |
| `incomplete` / `2` | Evidence, required behavior proof or current independent review is unavailable. |
| `invalid` / `3` | Malformed/unsupported content, duplicate identity, hash mismatch, unsafe reference or unsupported claim. |

`verified` is not sufficient for recurring mechanical enforcement: a one-off or guidance proposal can be valid on its own terms. Inspect `recurrenceAssessment`, `enforcementProof`, `disposition` and `for13Gate` separately. The gate requires at least two resolved original incident identities, bound independent review and resolved historical/legitimate/conflict proof. Missing private evidence on another machine yields `incomplete`; committed summaries do not silently replace it.

## Source provenance

| Occurrence | Original reference and extraction | Limits |
| --- | --- | --- |
| R8 proposal replay | `062e30b70fd9f50cb6eb02c99bc26b2239e7a529:docs/testing/mvp-progress.md`, line 438; Git blob `bff247c2e8dad3a57b7d0eca441de30351768dab`. [Exact retained excerpt](sources/r8-review-excerpt.md). | Checked-in independent-review summary only. Original reviewer transcript and pre-fix executable were not found. `affectedRevision` and exact observation time remain unknown; the pinned commit records the repaired incident. |
| MH6 package-report replay | Original `/tmp/for11-spec-review.md`, finding 3, lines 23–29; copied to private `historical-01/originals/for11-spec-review.md`. [Exact retained excerpt](sources/mh6-review-excerpt.md). Reviewed bad revision `2a9bf0c0f6c7f5ae24426e214974208527f5d7df`. | Original diagnostic/log are corroboration of the same event, not two additional occurrences. Its exit zero observed the defect; fresh inverted regression establishes enforcement. |

Both excerpts preserve the exact original UTF-8 bytes, including final newline; no rewriting or redaction was needed within the selected spans. The record binds the entire original source SHA-256 and selected span SHA-256. These readable copies are not substituted as independent incidents.

The remedy is the existing domain/API ordering, supported by behavioral tests. Types cannot determine whether a request already committed or whether mutable state subsequently changed. A generic static rule cannot establish the lifecycle ordering. No new production fix, structural ticket or ceremonial guidance proposal is claimed.

## Version 1 contract

The validator is the executable schema; unknown fields at every defined record, reference, proof and review level are rejected. Root `*.json` files each contain one record; duplicate class IDs are invalid. JSON rejects duplicate keys, nonfinite numbers (including exponent overflow) and malformed Unicode. Strings describing observations and rationale are required, but their truth remains subject to independent review.

| Field | Content |
| --- | --- |
| `schemaVersion`, `id`, `issue`, `repository` | Version `1`, stable class identity, `FOR-13`, `the-data-sherpa/crewroom`. |
| `invariant`, `scope` | The shared behavioral rule and its authentication/lifecycle boundaries. |
| `occurrences` | Nonempty list of `id`, `observedAt`, `affectedRevision`, `behavior`, `independence`, `source`. Unknown observation time or affected revision is `null`; known values use a timezone-bearing timestamp/full commit. Each occurrence needs an excerpt or JSON pointer. Duplicate IDs or relabelled identical source selections are invalid. |
| `remedy` | `primaryLayer`, `supportingLayers`, `rationale`, `strongerLayersConsidered` (`layer`, `reason`) and `existingEnforcementInspection`. Layers: `domain_api`, `types`, `static_analysis`, `behavioral_test`, `guidance`. |
| `enforcement` | `claim` (`mechanical` or `proposed`), source/check/historicalBadSource references, `symbol`, `legitimateBehavior`. Code/check/symbol fields may be `null` for a proposal. Guidance cannot claim mechanical enforcement. |
| `verification` | A proof reference or `null`, plus explicit `limits`. |
| `disposition` | `kind`: `existing_enforcement_reused`, `structural_work_proposed` or `judgment_guidance_proposed`; optional genuine `links` array. Each link has `kind` (`task`, `proposal`, `decision`, `revision`), `id`, `status`, `source`; the resolved source JSON must agree on `id` and `status`. A link is evidence metadata, never admission, execution or adoption. |
| `independentReview` | Reference to the review object below, or `null`. |
| `validationOutputs` | Prior generated-validation references, ordinarily empty. |

A reference is either `{kind:"git", revision:<full commit>, path:<relative path>, sha256:<full blob SHA-256>}` or `{kind:"private", path:<relative path>, sha256:<entire retained file SHA-256>}`. Private paths require an explicit evidence root. Paths must be normalized, relative and contained, with no traversal; resolved symlink escapes and Git symlink blobs are invalid. Git references resolve a regular blob at the pinned commit, independently of working-tree changes.

An optional `excerpt` is `{startLine:<1-based>, endLine:<inclusive>, sha256:<exact selected bytes>}`, spanning at most 200 lines. An optional `jsonPointer` is `{pointer:<RFC 6901>, sha256:<canonical selected JSON>}`. Only one extraction is allowed. Pointer arrays require an unambiguous canonical index; missing fields, invalid escapes and mismatched selections fail closed. Full-file hashes are always checked before extraction. Text/JSON extraction is bounded to 16 MiB; binaries are streamed for hashing.

## Proof and review binding

The selected proof object has exactly:

```text
schemaVersion: 1
kind: correction_enforcement
historicalBadRevision, currentRevision: full commits
historicalSourceSha256, currentSourceSha256, fixtureSha256
runs: [historical_bad, legitimate, payload_conflict]
cleanup: {status: complete, receipt: reference}
limits: [explicit limitations]
```

Each run contains `case`, nonzero equal `selected`/`executed` counts, integer `exitCode`, `outcome`, causal `assertion`, `sourceSha256`, and retained `binary`/`output` references. Historical bad behavior must report `expected_failure` and a positive exit; legitimate replay and changed-payload rejection report `passed` and exit zero. Their source and fixture hashes must match the enforcement mapping. A combined test may establish both legitimate and conflict assertions; these are two behavior mappings, not two executions. Outputs and cleanup resolve, while the independent review confirms that failure was causal and success covered the stated behaviors.

`claimSha256` is SHA-256 of UTF-8 sorted-key compact JSON (`ensure_ascii=False`, no nonfinite numbers), retaining array order and excluding **only** top-level `independentReview` and `validationOutputs`. Changes to the invariant, occurrences, remedy, disposition or proof invalidate the old review. Excluding references avoids circular hashes; it does not exempt any underlying claim.

The referenced independent review object has exactly:

```text
schemaVersion: 1
kind: correction_claim_review
classId, claimSha256
reviewer: {identity: <named reviewer>, independent: true}
reviewedAt: <timestamp with timezone>
occurrenceHashes: [{id, sourceSha256, selectionSha256}, ...]
proofSha256: <selected proof SHA-256, or null>
recurrence: confirmed | one_off | unresolved
enforcement: confirmed | not_claimed | unresolved
rationale: <why the evidence supports those conclusions>
limits: [explicit limitations]
```

The validator prints the claim/occurrence/proof hashes needed for a review. A separate reviewer must inspect the original incidents and the causal proof before issuing that artifact. `review: bound` means its conclusions and hashes match the current claim; the CLI cannot authenticate a reviewer identity or replace semantic judgment with a Boolean. A missing, stale or unresolved review remains `incomplete`.

For judgment-dependent advice, record a proposal disposition and leave mechanical enforcement unclaimed. S7 remains the operator-controlled proposal/decision path, with its own genuine citation rules and future-assignment versioning. This repository ledger does not import historical documents as fabricated product evidence IDs.
