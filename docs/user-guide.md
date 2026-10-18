# How to use Crewroom

Crewroom turns an accepted brief into agent assignments, checked code and a Bitbucket pull request. You direct the work and decide what to merge. Start with one small ticket whose acceptance criteria you can verify.

## Start the native workbench

Use the installed bundle and the same explicit data and role paths each time:

```sh
/path/Crewroom.app/Contents/MacOS/crewroom-launcher \
  --data-dir /absolute/private/data \
  --roles /absolute/private/config/roles.json
```

For a source checkout, see [build and start](../README.md#build-and-start). The launcher opens the native window and starts or reuses the background service. Reopen keeps the existing service configuration. Closing the window leaves the service and agents running. Finder alone does not provision the service.

A fresh workbench can save local tickets, briefs, roles and repositories. Agents need configured OMP, tmux, Rust Agent Mail, Docker and authenticated model access. Jira and Bitbucket need their own configuration; save their access tokens in **Settings → Integrations** (stored encrypted, never shown again). GitHub connects there with a personal access token that Crewroom checks with GitHub before saving; see [GitHub token permissions](operations.md#github-personal-access-token). A project's GitHub issues can be imported as tickets once GitHub Issues is its confirmed ticket source under **Settings → Repositories**, where the project's filters and **Sync issues** live; see [GitHub Issues ticket source](operations.md#github-issues-ticket-source). See [operations](operations.md#configuration-inventory); the app does not provide an account-setup wizard. Unexpectedly empty data usually means a different data directory was selected.

## Find your way around

| Area | Use |
| --- | --- |
| Tickets and Queue | Select work, order coding plans and inspect holds |
| Brief | Accept local requirements, select the target, queue work and adopt revisions |
| Conversation | Send coordinator steering, inspect delivery and propose specialists |
| Results | Inspect revisions, checks, review, citations, publication and recovery actions |
| History | Inspect prior runs, attempts, captured configuration and recorded usage |
| Open agent wall | Observe separate agent terminals and retained output |
| Agent roles | Create and edit reusable agent specialties, instructions and models |
| Repositories and Learning | Configure future work and review proposed guidance |

On a narrow window, use **Tickets & settings** to reach navigation. Jira status, workflow state and agent activity are independent. A queued plan has not necessarily started, and observed idle output does not prove validation passed. The agent wall is read-only; send direction through Conversation.

## Prepare and queue a coding ticket

1. In **Repositories**, register the existing checkout, target branch, instruction files and required checks. Each repository's row suggests its ticket source from the remote it publishes to (a github.com remote suggests GitHub Issues; a bitbucket.org remote suggests Jira, with a site you pick); confirm it once. If the remote later changes, the row warns that the source no longer matches and changes nothing; a source cannot be changed while queued or open work uses the repository. Checks require existing pinned local Docker images, absolute executables, arguments and timeouts; they run without network. Crewroom uses owned worktrees and leaves your checkout intact.
2. In **Agent roles**, choose **Edit** beside a specialty or **Create agent role** to add one. Set its purpose, instructions, model and reasoning effort, then **Save agent role**. Enabled roles are available for new assignments. Existing assignments keep their captured definitions.
3. Use **New ticket** for a local request, **Refresh Jira** in a Jira project's row under **Settings → Repositories** for configured live intake (each project saves its own project key, saved filter, assignee or JQL; see [Jira refresh per project](operations.md#jira-refresh-per-project)), or **Import snapshot** for supported Jira JSON. Snapshot import does not connect live Jira.
4. Select the ticket and open **Brief**. Choose its target repository, save the local brief and acceptance criteria, and resolve source-change or unsaved-draft holds.
5. Choose the queued coordinator role and select **Queue run**. Inspect the queue and hold reason. With valid launch details, available execution and a free slot, coding starts automatically.

Only one coding ticket executes at a time. Queue order is separate from Jira rank. Dependencies require provider-confirmed merge; a closed PR or local branch alone cannot release them. A changed target may require explicit reconciliation before the dependent plan can start.

## Direct active work

Open **Conversation**. Write direction and select **Send to coordinator**, or press Command-Enter. Crewroom first stores the message; receipt and application to the plan are separate recorded transitions. Applied steering does not certify completed work. Paused steering remains saved for later delivery. Inspect failed or uncertain delivery before sending another copy.

For changed scope, save the new brief in **Brief**, then review **Requirement adoption** and record the reason before choosing the intended revision. Existing run requirements do not change silently; earlier evidence may become stale.

In **Conversation → Add a specialist**, select an enabled agent role, describe its assignment and choose relevant dependencies/criteria, then **Propose specialist**. This works on queued coding plans as well as active coding or research runs, with the runner configured. Use **Configure agent roles** to create or edit a specialty, then **Back to ticket**; your unsent assignment stays in place. A pending proposal waits for a safe coordination point and, while paused, for Resume. It is not yet an agent process. A team admits one coordinator and up to three specialists.

| Control | Meaning |
| --- | --- |
| Pause run | Requests a safe stop; pausing remains visible until ownership is stopped |
| Resume run / Resume pending | Requests retained work; scheduling or recovery holds may delay admission |
| Cancel run… → Confirm cancel | Ends the run while preserving changes, evidence and any PR |
| Continue later | Creates an eligible linked successor using the verified retained branch/PR, with fresh evidence |

If cancellation left no owned branch, queue a new run from the accepted brief. Do not try to resume the cancelled run or change its database state.

## Inspect results and review feedback

Open **Results**. Inspect the candidate revision, acceptance coverage, actual check outputs and independent review. **View change** opens the saved diff. Historical evidence remains readable and applies only to its recorded revision. Missing, skipped, failed or stale required checks block readiness.

Local validation, PR delivery, hosted checks and human review readiness are distinct. Use **Open pull request** for your final review and merge. At the review gate, Crewroom pauses and releases the coding slot; it never merges for you.

Read PR feedback before choosing **Resume run**. Rework keeps the owned branch and PR identity but needs fresh checks and independent review. Use **Reconcile publication** or **Reconcile Jira updates** for uncertain provider outcomes. Target movement may offer **Refresh from target** or a reasoned **Retry from current target**; inspect the retained changes and hold first.

Open **History** for earlier attempts, requested/resolved models, fallback lineage and recorded usage. Unavailable observation does not establish a new agent failure. Actual, estimated and unavailable accounting remain labelled separately.

## Refinement, research and learning

**Explore** develops the request; **Grill** challenges it. Both preserve proposals and notes independently until you accept or discard a proposal. They require an available runner and do not queue coding by themselves. Use **Edit before accepting** when needed and record a reason when discarding or adopting changed scope.

**Queue research** captures the accepted brief and criteria. Choose the research coordinator role and **Start research coordinator** when execution is configured. Review citations, coverage and limitations in Results. Research does not need a fabricated coding commit, check or PR.

In **Learning**, compare proposed role or project guidance with the current version, record a reason and approve or reject it. Approval affects future assignments; current assignments retain their snapshot.

## Finish and reopen

Closing the app leaves the service running. To hold work, Pause it and verify stopped ownership. For full shutdown, follow [Start and stop](operations.md#start-and-stop): identify the exact selected daemon, send SIGTERM and wait for exit before stopping separately managed Mail. Starting a new build does not replace an already-running service.

Return with the same bundle/configuration/data paths. Inspect recovered state and holds before requesting Resume. Keep worktrees and runtime records intact. A full backup includes evidence, repositories, runtime sessions/receipts and Mail; the [same-path restore procedure](operations.md#backup-and-same-path-restore) explains the required boundaries.

If work is held, inspect the recorded reason first: missing runner configuration, queue dependencies, uncertain ownership, pending delivery or incomplete revision evidence need different remedies. See [status](status.md) and [failed-service recovery](operations.md#failed-service-recovery).
