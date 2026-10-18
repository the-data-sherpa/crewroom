# FOR-14 comparison evidence contract

This contract describes the bounded case pack and trusted scorer for [FOR-14](https://linear.app/forgeinnovations/issue/FOR-14/s93-compare-instruction-revisions-before-adoption). The [implementation plan](for-14-implementation-plan.md) and [S9.3 acceptance](../workbench-redesign.md#s93--compare-instruction-revisions-before-adoption) remain authoritative. Deterministic check results establish fixture and scorer behavior; they do not establish live instruction improvement or native UI acceptance. Current executed acceptance belongs in [MVP progress](mvp-progress.md).

## Frozen case inputs

The [version 1 pack](../../scripts/fixtures/for14-comparison/v1/case-pack.json) contains exact ordinary task prompts, project guidance, public source bytes, source hashes, required checks and their protected dependencies. The daemon embeds this JSON; it does not invoke TypeScript to create or execute comparisons. The [loader](../../scripts/rust/for14-case-pack.ts) regenerates and verifies the pack against its checked-in source. Changing any prompt, source, check or oracle invalidates the frozen pack. Executed comparisons retain their own immutable copy; changing the pack requires a new comparison.

`sourceHash` is SHA-256 of canonical JSON containing the path-sorted `{path, sha256}` source manifest. `promptHash` binds UTF-8 prompt bytes. Each `definitionHash` binds the check executable and its immutable local helper. The image digest binds Chromium, Playwright, Node and their other dependencies. A configured image must also prove that its `/opt/crewroom-checks/` bytes match the embedded pack before execution; an arbitrary image digest alone is insufficient.

| Neutral task | Ordinary contract | Required worker checks | Initial condition |
| --- | --- | --- | --- |
| `task-01` | Delivery labels and numeric validation | `unit`, `acceptance` | A genuine prior unit pass leaves the zero-day behavior uncovered. |
| `task-02` | Zero-based pagination, including zero and null | `unit`, `acceptance` | Source A passes; subsequent source B contains the cursor regression. |
| `task-03` | A useful empty-state helper with reasonable incidental wording | `unit`, `acceptance` | The initial helper returns empty text. Multiple sensible copy choices pass. |
| `task-04` | Immediate people filtering, announced empty state, Escape/focus and narrow layout | `unit`, `browser` | The pure function passes; page event wiring and layout still need work. |

Workers receive only the public workspace file list, ordinary task, applicable instructions and scoped tools. They do not receive private variant labels, the result rubric, oracle mapping or a prescribed tool sequence. `priorFiles` are materialized and actually checked before current files; historical receipts never count as worker activity. Task 2's source-A pass is not a fabricated text file or a pass attributed to source B.

Before each initial or replacement evaluation launch, the service compares the full generated tool catalog with the frozen catalog for that actor. Messaging is limited to the frozen peer name; authenticated backend calls also reject self-addresses and UUID aliases. Each successful host setup records the canonical JSON SHA-256 of the exact catalog supplied to its runner. The live helper checks that record against the manifest for every observed cell and independently checks Codex's retained session catalog. A missing or mismatched catalog keeps live acceptance unverified. Runner compatibility fixtures additionally inspect provider-facing schemas: Codex and Claude use their pinned function/MCP projections; OMP adds its pinned required intent string and removes it before calling host tools. All other compared schema fields must agree. Explicitly scripted provider fixtures may verify actual executables and production tools, but are rejected as live matrix profiles and contribute no live behavioral workers.

The private [oracle mapping](../../scripts/fixtures/for14-comparison/v1/private/oracles.json) selects the declared acceptance/browser check for an independent execution after worker ownership stops. It adds no secret behavioral obligation. The mapping and scoring implementation remain outside worker mounts. The final oracle measures source correctness and cannot supply missing worker verification.

The prospective terminal-handoff policy applies equally to role, principle and playbook comparisons. A valid structured `complete` or `incomplete` worker handoff ends the evaluated cell's agent execution and seals its source. The supporting coordinator does not have to finish its turn; a playbook comparison's evaluated sequence also ends at the worker handoff. After acceptance, neither actor may edit source, run further worker checks, send or dispatch new messages, advance the checklist or launch another agent generation. The accepted handoff and its source-bound finalization intent must survive restart together. Stopped ownership remains mandatory before the trusted final oracle and scorer run.

Handoff acceptance before the active-time limit is not downgraded because cleanup or independent final assessment takes additional time. A later operator Pause or Cancel blocks future cells while the accepted cell completes its bounded trusted assessment on the sealed source. This does not convert an `incomplete` handoff into a completion claim, make unavailable oracle evidence available, or permit new evaluated work. These are prospective semantics for the terminal-handoff repair; the historical incomplete matrix results remain unchanged, and fresh validation is required before claiming acceptance.

Cancellation remains pending while any admitted cell lacks a terminal result, including a preparation-only cell with no agent attempts. Its shared coding slot stays reserved until the existing result transaction releases it. Preparation rereads the comparison's control intent before launch and after admission-error cleanup: Pause remains paused, and Cancel settles through normal ownership reconciliation instead of becoming provider unavailability. A daemon interruption may preserve pending cancellation for later reconciliation; physical process cleanup alone does not establish terminal comparison state.

The acceptance helper independently requires terminal results for every run-bound cell, stopped ownership, terminal run states and a released coding slot before reporting settled cleanup. An older daemon's premature cancelled aggregate cannot satisfy that check. This is separate from the stopped-process proof retained for historical trials.

## Protected execution and scoring

The [check image recipe](../../scripts/fixtures/for14-comparison/v1/Dockerfile) installs protected definitions under `/opt/crewroom-checks/`. Ordinary checks import only Node builtins and the immutable helper; workspace code executes in a separate child so it cannot replace the parent's assertions or pass by exiting early. Runtime checks use the existing `ContainerCommand`: network disabled, read-only source and root, dropped capabilities, bounded resources, private temporary storage and the local owner's UID. The image build helper verifies every protected file hash using the built image itself.

Successful logic checks emit a JSON observation containing version, check identity, surface and an executed observation. The browser check emits verified actions and screenshots at 1280px and 390px. Each PNG is bounded to 1 MiB and includes its byte count, SHA-256 and base64 bytes inside the durable output artifact. Chromium launch unavailability exits 70; a completed behavioral failure exits 1. Neither is a passing worker check.

The production [Rust scorer](../../crates/crewroom-runtime/src/supervisor/workflow/evaluation/scoring.rs) receives cell-bound runtime events, final source, structured handoff and the independent oracle. The runtime reads output through the artifact store before invoking the scorer. The scorer also checks the retained body's hash and byte count. It requires exact source and definition hashes, actual start/completion, a successful exit, intact source, the correct observation schema and, where applicable, browser actions plus bound screenshots at both widths. An available revision object must match in full. `run_shell` output and positive prose are not check receipts.

Results keep these facts separate:

- Source correctness is true, false or unavailable. A missing, interrupted, stale or infrastructure-unavailable oracle stays unavailable.
- Required checks identify each passing receipt by its retained output artifact ID and SHA-256. Historical checks and independent oracle receipts are excluded.
- Surface exercise requires the actual browser observations for the browser task; pure-function tests cannot supply them.
- Completion claim and false completion remain separate. A positive structured handoff with absent required evidence is false completion. Honest incomplete work is unsuccessful without inventing a false claim.
- Ambiguous or contradictory prose requires evidence-backed independent adjudication. The scorer does not search for reassuring keywords; unknown classification stays unknown.
- The outer cell result retains attempts, failed checks, edits, operator events, elapsed active time, usage availability and stopped ownership. Scoring does not replace these records or fill unavailable usage with zero.

Source changes and renewed passing verification after a failure are repair candidates. Whether a change was substantive, or an operator decision was unnecessary, requires independent evidence-backed classification; a hash difference or message count alone does not establish either. Launch, fixed pause drills and infrastructure failures must remain distinguishable from task interventions.

## Verification recipes

Use Node 24 or newer on `PATH`, as required by `package.json`.

```bash
node --version
node --import tsx --test scripts/rust/for14-case-pack.test.ts
cargo test -p crewroom-runtime --lib evaluation::scoring
node --import tsx scripts/rust/for14-check-image.ts .crewroom/test-results/for14/check-image-new
```

Use the returned immutable digest for actual contained checks:

```bash
FOR14_EVALUATION_IMAGE=sha256:<verified-image> \
FOR14_CHECK_RETENTION=.crewroom/test-results/for14/protected-checks-new \
node --import tsx --test scripts/rust/for14-protected-checks.test.ts
```

The protected checks retain a receipt for every success and failure, including stopped container ownership and removal. The initial local browser test exposed a helper environment error: it omitted `HOME=/tmp`, which production `ContainerCommand` already supplies. Both the failed receipts and the corrected helper's results were retained; this was not a worker behavioral failure. The fixture now also distinguishes unavailable browser startup from behavioral assertions.

The [live wrapper](../../scripts/rust/for14-run-live.py) copies exact native binaries, records source and binary hashes, starts fresh external Rust Mail and calls the [production service helper](../../scripts/rust/verify-comparison-workflow.ts). Its `sourceIdentity` is a launch-time checkout observation, not proof that the copied binaries were compiled from those files. Executable hashes must be linked separately to their retained build/package evidence, especially when reusing earlier binaries. It never launches a separate evaluation scheduler or posts its own scores. Private model settings are read from an owned mode-0600 file; they are not printed or committed.

```bash
python3 scripts/rust/for14-run-live.py \
  --harness codex --mode preflight \
  --harness-config /path/to/private-harness.json \
  --check-image sha256:<verified-image> \
  --output .crewroom/test-results/for14/preflight-new
```

`preflight` starts no evaluated workers. `smoke` starts a real production comparison, requires an observed worker handoff and worker check, then cancels the remaining cells with evidence intact. It is profile evidence, not matrix acceptance. Run it for each exposed harness. `recovery` separately requires a finished cell and real edit or check activity in a later cell before pausing. That specific later cell must remain unfinished when Pause stops its owners; a race with completion fails the intended recovery seam. The drill retains its event and attempt identities, restarts the daemon, verifies the same frozen manifest, finished-result hashes and remaining active budgets, then requires an observed result and new worker-generation tool activity on that same cell.

Before the first recovery run, its initial observation limit was set to 480 seconds to include two serial setup periods of about 100 seconds each; its resume observation limit remains 300 seconds. The native manifest retains 180 seconds per cell and 48 minutes aggregate active time. A separate recovery-only wrapper bounds execution to 780 seconds with at most 120 seconds for owned cleanup, conservatively bounding accepted recovery activity to 15 minutes. The stopped final active ledger must be present and at most 900,000 ms; missing evidence, exceeded limits or unsuccessful cleanup fail acceptance. A stop-request latch prevents a deadline from being followed by daemon replacement, Resume or a success claim, and cleanup waits for any already-sent control request before cancelling. Other profile modes retain their 900-second wrapper limit. Each receipt records the distinct native, observation, external execution and cleanup limits.

A held comparison normally fails the acceptance helper immediately. The prospective helper permits one bounded observation when the exact generic host-tool reconciliation reason occurs after an accepted handoff: one unfinished held cell must have exactly one matching handoff and durable finalization intent bound to the same source. It preserves the first held snapshot and reads until that same cell has a source-matching durable result, with one 90-second deadline that never resets across subsequent states. It performs no Resume, retry or intervention. An unsealed cell, changed identity or source, another held reason, or an expired deadline fails immediately. The original matrix03 wrapper failure remains retained; these observer tests and prospective behavior do not rewrite it as a passing execution.

Every smoke audits the retained `host_ready` hash of the actual catalog supplied to the runner against the frozen coordinator or worker catalog. Codex also retains its actual dynamic tool definitions in the session rollout; those names, descriptions, schemas and order are independently compared with the manifest. The matrix requires a passing catalog audit in its smoke receipt and audits each finished cell. A matching tool-name list alone is insufficient.

`matrix` requires `--tool-profile-receipt` from a passing smoke with an actual catalog audit covering the same native binaries, case pack, harness and check image. Before saving a proposal and again before creating the comparison, its runner identities, coordinator runner identity, models and tool catalogs must exactly match the new preflight. Isolated application paths may differ; binary hashes bind their contents. It retains four cases × two repetitions × two variants, exactly 16 distinct evaluated assignments, all attempts and two fixed actors per cell. Only ordinary task prompts are used; the helper does not patch workspace code, inject task coaching or retry a failed cell. The service enforces 180 active seconds per cell and 48 comparison-wide active minutes. The new trial uses an 85-minute helper wall limit and a 90-minute outer wrapper deadline, followed by owned cleanup. These limits were set before the new matrix: retained completed cells show about 100 seconds each of setup and final verification outside the active ledger, so the full 48-minute active allocation needs about 75 minutes of wall time. The earlier 55/60-minute limits remain part of the aborted first trial; no executed trial receives a mid-run extension.

Pass `--binaries <first-smoke-directory>/bin` for subsequent profiles, recovery and matrix runs to reuse the exact retained binaries even if the development checkout is rebuilt. Each attempt still receives its own immutable copy and private state directory. Keep evaluated workers serial across these acceptance runs.

The first candidate appends the implementation plan's verification/discretion paragraph through the existing operator S7 proposal API in isolated state. The helper reads actual saved roles if configured; otherwise it explicitly records the effective built-in default rather than claiming a custom saved revision. Model selection uses isolated global policy and leaves role fields equal across variants. No instruction adoption occurs and the user's live catalogs remain unchanged.

A complete failed behavior is a legitimate observation. Unexecuted/provider-unavailable cells do not meet the 16-worker acceptance gate. Results use two observations per variant per case and retain incomplete coverage. Any conclusion is descriptive for this bounded trial; it does not establish statistical superiority, equivalence or a general reduction in operator work. Crewroom's native editor/results still require their own GPUI and real native visual acceptance; the browser fixture supplies neither.
