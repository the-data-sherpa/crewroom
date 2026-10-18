# MH5 intake contract (FOR-10)

Implementation contract, 2026-10-13. This extends new repository-job intake;
standalone Explore, Grill and role drafting remain available. Pair choreography
belongs to FOR-11. Source changes do not update the active installation.

| Input/state | Durable result | Execution authority |
| --- | --- | --- |
| Save a ticket | Saved idea | None |
| Queue, current accepted plan | Coding queue entry with frozen plan | Existing queue, dependencies, model and ownership gates |
| Queue, absent/stale plan | Intake intent and one coordinator-only Grill | Bounded scoping only |
| Questions/proposal recorded | Stop requested, then Waiting for you / Plan review | No coding assignment; stopped wait consumes no active time |
| Answer | Saved answer and keyed steering receipt | Resume scoping only after stopped ownership |
| Accept current plan | Operator decision and handoff intent | Close stopped Grill before creating one coding successor |
| Cancel intake | Revoked queue intent; retained history | Late acceptance cannot revive it |
| Requirements/source/instructions change | Needs revalidation | Old decisions remain visible; new coding admission held |
| PR feedback | Existing explicit Resume hold | Intake never auto-resumes owned coding work |

The service remains the sole scheduler. At most one automatic intake coordinator
executes at once. Stopped operator waits hold neither the coding slot nor a writer.
Queue intent retains its original order through the handoff. An active coding run
retains its existing ownership and admission contract.

Plans are immutable, schema-versioned records of goal, scope, exclusions,
assumptions, decisions and named work packages. Packages identify stable IDs,
dependencies, outputs, criterion indices and validation obligations. Cycles,
dangling/duplicate IDs and uncovered criteria are invalid. Proposed packages are
not assignments. Acceptance is a separate keyed operator decision against the
displayed task and pinned source basis. An edited or imported plan is another
draft and needs acceptance. Optional requirements adoption and plan acceptance
are atomic.

Source inspection reads bounded text from the selected repository's pinned Git
commit, with tree and instruction identity. It cannot read untracked files, run
shell commands, edit files, claim a writer, publish, or mutate Jira. The capability
belongs only to the current coordinator of the linked intake. Git validation runs
outside the workflow lock; saving the proof rechecks captured task/configuration.
Repository text supplies context, not tool authority.

Currentness covers accepted requirements, repository configuration, target
commit/tree and instructions. Source movement requires explicit revalidation;
previous answers remain available. Notes, Jira status alone, and model refresh do
not accept or invalidate a plan. Existing Jira requirements-reacceptance rules
still apply. Model eligibility is checked separately at actual admission.

Migration adds records without starting agents or accepting plans. Legacy queued
jobs without an admitted assignment need a plan; already-admitted runs retain
their history and recovery authority. New cancelled-run successors meet the new
admission gate. Rollback uses a stopped whole-state backup and its matching binary.

Acceptance evidence is recorded in `mvp-progress.md` and the MH5 receipt. This
contract describes required behavior, not an executed acceptance claim.
