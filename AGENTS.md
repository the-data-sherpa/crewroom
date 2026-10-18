# Crewroom

On the maintainer’s machine, use the existing Crewroom checkout on `main` as the single development checkout; disposable worktrees for isolated verification remain allowed.

## Public repository

Treat this repository and everything committed or posted to its GitHub issues and pull requests as public. Before committing or uploading artifacts, inspect the actual content for credentials, private account/customer/employer information, internal source or ticket details, and identifying logs or screenshots. Use synthetic examples and redact retained evidence without claiming it proves more than it does.

Keep runtime stores, provider configuration, raw agent sessions, private backups and scan reports outside tracked files. `.gitignore` does not protect content already tracked; never force-add private state such as `.crewroom/` or `.env` files. If private content is found, stop the affected publication, report the location without repeating the content, and remove it from the publication candidate before continuing.

Public history starts from a reviewed source snapshot. Never merge or push branches, tags, stashes, bundles or Git metadata from the old private history into this repository. For historical revision references and publication checks, read [the publication record](docs/publication-review.md).

## Linear task tracking

Linear project [CrewRoom](https://linear.app/forgeinnovations/project/crewroom-968b4d67bed7) is the authoritative tracker for this repository's tasks, priorities, progress and blockers. Project: `P-FOR-1`; UUID: `d8e8dc0c-c48a-4f3d-83c6-ccb5b1a2c17e`; team: `FOR`. Use Linear MCP for reads and writes. Jira remains a Crewroom product integration.

1. **Pull current context before starting or resuming work.** Read the project's issues, following pagination as needed, then the selected issue's full description, acceptance criteria, status, dependencies and latest comments. Refresh the selected issue at meaningful checkpoints and before completion. Incorporate new operator decisions; reconcile conflicts with the accepted product/architecture contracts before changing affected scope.
2. **Track the work before implementation.** Reuse the matching issue. If none exists, create a bounded issue with scope and acceptance criteria, explicitly setting the project UUID above and team `FOR`. Keep every new repository issue, including follow-ups and sub-issues, in CrewRoom. Reference its identifier in commits and handoffs.
3. **Update Linear as the task progresses.** Discover the team's available workflow states. Move the issue to the appropriate active state when execution begins; record meaningful milestones, scope changes, blockers, pauses and handoffs with completed work, remaining work and the next action. Keep dependencies accurate and use the available state that truthfully describes progress.
4. **Preserve others' updates and verify synchronization.** Re-read the issue and relevant comments before writing; update only intended fields and use targeted description edits. After each create or update, fetch the issue or comment again and confirm the intended content, state and project membership. If a write times out or its outcome is uncertain, read back before retrying; search for an existing issue before repeating creation.
5. **Close with evidence.** Record the result, exact commit/PR, executed checks, limitations and any follow-up issue links. Mark Done only when the issue's acceptance criteria and required validation are satisfied; otherwise retain the appropriate active/review state and explain what remains. Verify the final state through MCP before reporting completion to the user.
6. **Keep repository records consistent.** `docs/status.md` records implementation and validation state; `docs/testing/mvp-progress.md` holds revision-bound evidence. Link these from Linear where relevant and update them when implementation or accepted direction changes. Linear owns task tracking; repository plans retain accepted dependencies and acceptance gates.
7. **Handle unavailable MCP explicitly.** Record pending updates in `docs/status.md` with the issue identifier (or intended title/project), unsent content and failure reason. Report synchronization as pending. Continue work that does not depend on missing context, then reconcile and verify the pending updates when MCP returns; clear the pending marker only after confirmed delivery.

## Product and implementation contracts

Read `docs/status.md` for current implementation and validation status (`HANDOFF.md` is only a pointer and resume prompt) and `docs/product.md` for accepted product direction before continuing implementation. The user approved the shared design on 2026-09-25; `docs/discovery.md` records the completed interview. `docs/architecture.md` contains the approved implementation direction and first-milestone acceptance cases; revalidate affected contracts when changing the selected runtime. Use `CONTEXT.md` for domain terminology.

- Jira status, workflow state, and agent activity are separate facts. Keep them separate in data and UI.
- A queued plan is not an executing agent. Display unavailable integrations and undelivered messages accurately.
- Keep credentials server-side. The local server binds to loopback and accepts mutations only from its own origin.
- Persist ticket refinements and user steering independently of imported Jira fields.
- Review and test evidence must identify the revision they cover. A quiet terminal is never a quality gate.
- Put reusable role definitions in the user configuration; project instructions constrain each assignment.
- Keep the core workflow independent of NTM. Add a runner adapter only when its contract is tested against a real runner.
- Run the coordinator and each specialist in separate, externally observable OS processes; route messages outside OMP's internal team bus. See `docs/architecture.md` for the supervision and message contract.
- Verify domain changes with `npm test`, type/build changes with `npm run build`, and user flows with `npm run test:e2e`.
- For verification of steering across reconnect, pause/resume, or readiness after revision changes, start from [the feature map](docs/testing/feature-map/README.md); use its versioned recipes, isolated state and retained cleanup receipts.

The first milestone is a narrow live Jira-to-PR workflow with supervision and interruption recovery, using the minimum persistent workbench it needs. Follow `docs/mvp-plan.md` in work-item order and use `docs/testing/mvp-progress.md` for executed evidence. Test each item before advancing; preserve the distinction between simulated provider checks and live delivery. Inspect `docs/research/omp-team-reuse-assessment.md` before rebuilding equivalent workflow, evidence, or recovery mechanisms; keep the retired installation inactive.
