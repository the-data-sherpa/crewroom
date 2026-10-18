# Existing routing: relevance to Crewroom

Inspected 2026-09-25 after the user raised JEV or a System 1 model as possible specialist-selection aids. This is source/document inspection, not a live model test. No model, credentials, private runtime journal, or configuration file was accessed.

## Distinct decisions

| Decision | Existing local behavior | Crewroom implication |
| --- | --- | --- |
| Dispatch mode | Archived request classifier chooses direct work versus workers plus a quick/standard/code/deep capability hint | Does not supply a specialist roster |
| Specialist selection | Classic OMP coordinator guidance describes selecting the next useful specialist by need and retaining suitable workers | Useful workflow guidance, separate from the retired queue's fixed phases |
| Model assignment | Named/configured roles supply worker models; the documented native task interface did not permit per-task model overrides | Validate model assignment against the selected runner; do not assume arbitrary overrides work |
| Next action and continuity | JEV recommends an immediate action and whether to continue existing context; local policy maps that to a fixed role vocabulary | Possible future adviser; does not yet support Crewroom's arbitrary user-created roles |

The archive [request-routing extension](/Users/crewroom/.local/share/omp-retired/20260925T180705Z/global-before/extensions/request-routing.ts:107) supplies mode/capability routing. The [classic worker guidance](/Users/crewroom/.local/share/omp-retired/20260925T180705Z/global-before/docs/worker-workflow.md:11) describes adaptive dispatch; model handling is documented at line 30 and next-phase selection at line 58. These are distinct from the fixed serial OMP team queue assessed in [omp-team-reuse-assessment.md](../research/omp-team-reuse-assessment.md).

## JEV evidence boundary

The archive points to the active JEV workflow documentation. Local [runtime-context source](/Users/crewroom/.omp/agent/lib/jev-runtime-context.ts:3) connects JEV to a TypeSafe System One endpoint. This explains the local relationship between those names, but does not prove what alternative provider/model the user may intend or select one for Crewroom.

The [shadow workflow documentation](/Users/crewroom/.omp/agent/docs/jev-workflow-shadow.md:69) and [policy code](/Users/crewroom/.omp/agent/lib/jev-workflow-policy.ts:5) describe next-action/continuity advice. Ordinary shadow recommendations do not dispatch work. The [role vocabulary](/Users/crewroom/.omp/agent/lib/jev-workflow-state.ts:2) is a fixed enum.

A separate [read-only pilot](/Users/crewroom/.omp/agent/docs/jev-readonly-pilot.md:3) limits execution to a finite set of fresh local retrieval/investigation dispatches with restricted workers/tools. It excludes Jira/API retrieval and leaves implementation, review, acceptance, and publication with the existing workflow. This inspection does not establish that the pilot succeeded or that broader autonomous routing is validated.

## Current Crewroom gaps

[RoleCatalog.update](/Users/crewroom/Projects/Git/crewroom/server/roles.ts:46) only updates an existing role ID. The defaults include a technical writer, but no separate documentation/Jira role. [Run](/Users/crewroom/Projects/Git/crewroom/shared/types.ts:35) snapshots roles and string stages without modeling specialist assignments, insertion, dependencies, or execution ownership.

The user's requirement to create roles and insert specialists therefore requires additional behavior. Exact responsibility boundaries and insertion timing remain interview decisions.

## Recommendation

Start with a configured coordinating model selecting from the user's role catalog. Keep model assignment and next-action/continuity advice separate from selection. A future adviser can propose an action while Crewroom validates that it refers to an available role, fits the assignment constraints, and respects required gates. This is a design recommendation, not adoption of the current JEV implementation or its model settings.
