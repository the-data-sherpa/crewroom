# Retired OMP team: reuse assessment

Follow-up after design approval: [the first contract experiment](omp-contract-experiment.md) records 13 passing retained cases and limited live RPC control observations. The source-inspection findings below retain their original scope.

Inspected 2026-09-25 at the user's request. This is a static source and test review. No archived code was executed, tests rerun, credentials read, or installation restored.

## Source identity

Archive: `/Users/crewroom/.local/share/omp-retired/20260925T180705Z`.

The six Python modules in `installed/.omp-team-0.4.0-7d3dff00c0b415df` match the archived `terraform-pilot-source/pilot/live/src` modules and `terraform-pilot-source/pilot/vendor/queue_ledger.py` byte for byte. SHA-256 comparisons were performed during this inspection. The earlier prototype at `pilots/omp-team-queue/src/queue_ledger.py` is a different version; use the 0.4.0 sources for this assessment.

Historical installation instructions describe the former environment. The archive's README identifies it as inactive material with stale absolute paths.

## Useful existing mechanisms

| Mechanism | Implemented behavior | Candidate use in Crewroom |
| --- | --- | --- |
| Durable workflow ownership | SQLite transactions, exclusive controller lock, unfinished attempts marked uncertain on reopening | Persistent run/attempt history and ownership that survive process failure |
| Revision-specific evidence | Candidate identified by base, HEAD, and tree; evidence copied by content hash; receipts retain producer, session, attempt, and candidate | Evidence displayed against the exact work it covers; rejection of stale review and test results |
| Executed checks | Controller runs configured commands and records exit status, timeout, output, and hashes | Verifiable local test results, independently of agent claims |
| Completed-phase continuation | Passing completed phases can be skipped on continuation; saved review evidence can sometimes be recovered | Avoid repeating completed work after interruption |
| Publication reconciliation | Persist intent before publication; distinguish an existing PR, confirmed absence, and unknown outcome | Avoid duplicate publication after a lost response; potential pattern for Jira writes |
| Work isolation and status | Managed worktrees, pinned bases, durable request/profile, status reads during execution | Inspectable agent work without conflating pending plans with execution |

Source references:

- [Ledger ownership and reopening](/Users/crewroom/.local/share/omp-retired/20260925T180705Z/terraform-pilot-source/pilot/vendor/queue_ledger.py:104); transactions at line 153.
- [Candidate identity](/Users/crewroom/.local/share/omp-retired/20260925T180705Z/terraform-pilot-source/pilot/vendor/queue_ledger.py:59), evidence snapshots at line 276, receipt provenance at line 357, and publication gate checks at line 526.
- [Check execution](/Users/crewroom/.local/share/omp-retired/20260925T180705Z/terraform-pilot-source/pilot/live/src/local_smoke.py:548) and candidate/command validation at line 567. Ledger receipts depend on a trusted caller; retain the execution boundary that supplies them.
- [Completed-phase continuation](/Users/crewroom/.local/share/omp-retired/20260925T180705Z/terraform-pilot-source/pilot/live/src/local_smoke.py:901) and saved-review recovery at line 756.
- [Publication intent](/Users/crewroom/.local/share/omp-retired/20260925T180705Z/terraform-pilot-source/pilot/vendor/queue_ledger.py:543), reconciliation at line 589, and [explicit absence reconciliation](/Users/crewroom/.local/share/omp-retired/20260925T180705Z/terraform-pilot-source/pilot/live/src/omp_team.py:649).
- [Managed worktree admission](/Users/crewroom/.local/share/omp-retired/20260925T180705Z/terraform-pilot-source/pilot/live/src/omp_team.py:398) and status reads at line 707.

## Gaps and coupling

The queue invokes `omp --print --mode json --no-pty`, with closed input and captured output. It does not inject commands into terminal panes. Reuse still depends on OMP's CLI and session/journal formats, which need a current runtime contract test. See [launch arguments](/Users/crewroom/.local/share/omp-retired/20260925T180705Z/terraform-pilot-source/pilot/live/src/local_smoke.py:494), child process handling at line 243, and journal validation at line 310.

The phases are fixed and serial: impact, implementation, documentation, review, then command checks. Model/vendor choices are hard-coded in the queue, and the ledger permits only one active implementation. These policies do not establish an adaptive specialist team. See [phase definitions](/Users/crewroom/.local/share/omp-retired/20260925T180705Z/terraform-pilot-source/pilot/live/src/local_smoke.py:35), [model mappings](/Users/crewroom/.local/share/omp-retired/20260925T180705Z/terraform-pilot-source/pilot/live/src/omp_team.py:35), and [implementation ownership](/Users/crewroom/.local/share/omp-retired/20260925T180705Z/terraform-pilot-source/pilot/vendor/queue_ledger.py:326).

There is no live operator-steering and acknowledgement contract in these modules. Requests have immutable objectives and criteria; changing an existing request is rejected. Switching workflow mode only stops subsequent task selection. It does not interrupt the selected task. See [request schema](/Users/crewroom/.local/share/omp-retired/20260925T180705Z/terraform-pilot-source/pilot/live/src/omp_team.py:117), request identity at line 320, and selection boundary at line 583. Archived documentation also describes native OMP worker messages for a separate classic workflow; that is not evidence that this queue supported them.

Recovery is partial. Completed work can continue, but unknown native outcomes require an audited reconciliation step, repair is limited to one cycle, and a changed target base requires a new task ID. Crewroom needs explicit behavior for steering that changes requirements, stale evidence, stalled workers, automatic recovery, and escalation. See [unknown-result handling](/Users/crewroom/.local/share/omp-retired/20260925T180705Z/terraform-pilot-source/pilot/live/src/local_smoke.py:850), refusal to repeat unknown work at line 897, and [target drift handling](/Users/crewroom/.local/share/omp-retired/20260925T180705Z/terraform-pilot-source/pilot/live/src/omp_team.py:623).

Jira synchronization and writeback are not implemented in the inspected queue/provider modules. Provider publication code covers GitHub and Bitbucket; Jira still needs its own contract.

## Existing evidence and tests

Preserve these fault scenarios when adapting the implementation:

- [Lost PR creation response](/Users/crewroom/.local/share/omp-retired/20260925T180705Z/terraform-pilot-source/pilot/live/tests/test_omp_team.py:201): adopt the existing PR without creating another; empty readback remains uncertain.
- [Interrupted native execution](/Users/crewroom/.local/share/omp-retired/20260925T180705Z/terraform-pilot-source/pilot/live/tests/test_omp_team.py:288): preserve the unknown outcome and block unsafe continuation.
- [Status and inbox during execution](/Users/crewroom/.local/share/omp-retired/20260925T180705Z/terraform-pilot-source/pilot/live/tests/test_omp_team.py:304): allow observations while maintaining exclusive execution ownership.
- [Crash after a durable phase receipt](/Users/crewroom/.local/share/omp-retired/20260925T180705Z/terraform-pilot-source/pilot/live/tests/test_omp_team.py:474): continue without another completed-phase model call.
- [Failed local checks](/Users/crewroom/.local/share/omp-retired/20260925T180705Z/terraform-pilot-source/pilot/live/tests/test_omp_team.py:190): prevent publication and bound repair attempts.

The archived [live results report](/Users/crewroom/.local/share/omp-retired/20260925T180705Z/terraform-pilot-source/pilot/live/RESULTS.md:21) reports real GitHub task publication and hosted checks. It explicitly limits interruption/recovery and merge scenarios to deterministic tests. The [Bitbucket report](/Users/crewroom/.local/share/omp-retired/20260925T180705Z/terraform-pilot-source/pilot/live/ONBOARDING-RESULTS.md:19) records read-only live validation and says team-created publication and current positive CI were not proven by a live write trial. These are historical reports, not fresh validation or verified current remote state.

## Recommendation

Reuse the evidence, ownership, publication, and recovery rules and their fault scenarios. Evaluate selective code extraction against Crewroom's agreed supervision contract before deciding whether to retain a Python component or port selected mechanisms. Keep role/model policy and OMP transport separate from the workflow rules. Revalidate extracted behavior in an isolated fixture and against the selected live runner before claiming it works in Crewroom.

The user has requested reuse where useful. No whole-engine adoption, implementation-language decision, old-state migration, or installation restoration has been selected.
