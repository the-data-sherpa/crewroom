# Local operations

For ticket preparation, agent supervision, and PR review, see [How to use Crewroom](user-guide.md).

Native operations use the GPUI app and the persistent Rust daemon. Component installation, restore, launcher and matching-old-build rollback checks are recorded separately in [native operations](testing/rust-native-upgrades-operations-2026-10-04.md), [launcher evidence](testing/rust-native-upgrades-launcher-2026-10-04.md) and [rollback evidence](testing/rust-native-rollback-2026-10-04.md). Final release acceptance must identify the combined source and exact signed bundle. The retained operator-merged pilot remains paused and outside repository startup cutover.

## Intake and implementation plans

Save retains an idea without starting a process. Select a repository and Queue to request scoping; a brief and criteria can be developed during intake. If a current accepted plan already exists, Queue skips Grill. Otherwise the workbench shows Scoping, then Waiting for you or Plan review after the coordinator stops. Scoping has no coding slot, writer, shell or publication authority.

Answer the displayed questions and choose **Send answers and continue scoping**. Saved answers and delivery receipts are separate facts. A stopped wait survives daemon restart without charging active time. Review the goal, work packages, criterion coverage and any proposed requirements before **Accept plan**. Acceptance can release the original queued request when launch details, dependencies, model availability and ownership permit it. **Request revision** keeps the reason and returns to scoping. **Cancel queued request** retains the dialogue and plans while revoking that request.

Use **Write a plan** or **Edit as a new plan version** for manual drafts. Saving does not accept a draft. Revision conflicts retain local edits; inspect the changed basis before choosing **Use current basis for this draft**. Imported API drafts use the same schema and operator acceptance. A source change requires **Refresh repository basis**, then stopped **Revalidate using this basis** for active intake. Previous answers stay visible. A queued coding run keeps its captured plan; if it becomes stale, cancel that unadmitted request and queue a newly accepted plan. Legacy queued jobs without a captured plan remain held until explicit plan acceptance.

Schema 29 adds the versioned pair/package ledger; schemas 27 and 28 remain additive predecessors. Stop the service and owned work before taking a whole-state backup, including configuration, sessions, artifacts, worktrees and journals. Roll back by restoring that backup at its original paths with the matching binary; an older binary must reject schema 29. The retired TypeScript authority remains schema 26 and must never open current native state. Browser compatibility tests exercise retained serialization separately.

## Supported environment

The accepted packaged release targets Apple Silicon macOS; an additional Arch Linux / Wayland source-build path is documented in the [Linux guide](linux.md), with separate validation limits. macOS source builds require Apple Command Line Tools, Git, Rust via rustup and Python >=3.12. The checked-in toolchain selects Rust 1.97.1; bootstrap verifies the host's Zig 0.16.0 archive. The packaged native app/service require neither Node nor build tools. Linux packages retain host shared-library requirements. Node >=24 remains necessary for the retained TypeScript validation tools; recent fixtures use Node 26.10.0.

Execution additionally requires a configured runner, tmux, separately managed Rust Agent Mail and Docker for contained commands. OMP uses the existing provider gateway; [Codex](testing/mh2-runtime-acceptance.md) and [Claude Code](testing/mh3-runtime-acceptance.md) use their existing native logins and declare different accounting/budget limits. Explicit native runtime/catalog configuration determines eligibility in the MH4 model picker. Actual native profile fixtures characterized OMP 18.4.12 and Rust Agent Mail v0.3.36. Other component versions and exact final-machine conditions belong in the final release receipt. The Jira loader uses macOS Keychain and `/opt/homebrew/bin/acli`; Bitbucket supports a server-side OAuth token or the `bkt` `conductor` Keychain context. General account onboarding and other platforms are not established by these fixtures.

After upgrading runtime dependencies, rerun their affected contracts. Keep the retired OMP team installation inactive.

## Start and stop

Build and package with `./scripts/run-native.sh --release --build-only`, then retain the printed immutable bundle path and its adjacent `.app.receipt.json` with final signed hashes. For installed use:

```sh
/path/Crewroom.app/Contents/MacOS/crewroom-launcher \
  --data-dir /absolute/private/data \
  --roles /absolute/private/config/roles.json --port 4318
```

Add `--headless` to start/reopen only the service. The data and role parent directories must be owned by the current user with no group/other permissions; an existing role file must be a private regular file. The launcher creates missing directories under a private umask. Finder does not start or configure the daemon.

A successful reopen reuses the authenticated service for that data directory. Its existing binary, roles, runtime configuration and port remain in effect. A new bundle or changed environment takes effect only after that exact service has stopped and restarted. Keep the same port across a restart intended to reattach live hosts. The loopback URL is an authenticated native API, not a browser workbench. Each data directory admits one controller.

Closing the app or launcher leaves the daemon running. To stop work for the day, Pause the selected runs and verify stopped ownership. Check the selected service with:

```sh
/path/Crewroom.app/Contents/MacOS/crewroomd --data-dir /absolute/private/data --status
```

Inspect the reported PID's executable, owner and start identity before sending SIGTERM to that exact daemon. Wait for it to finish its owned cleanup and exit. Stop separately managed Mail only afterward if needed. Ctrl-C in the already-finished bundled launcher does not stop the background service. A lost connection or quiet terminal does not prove process death. Never stop a daemon merely because repository startup defaults changed.

For source development, `./scripts/run-native.sh` builds the debug app. `npm run dev` and `npm start` are convenience aliases for debug/release native source launch. They rebuild; use the bundled launcher for ordinary reopen.

Pause retains a resumable run. Cancel is terminal; Continue later creates an eligible linked successor with fresh admission and evidence. Do not rewrite a cancelled run into a paused run.

## Lingering test processes

From this checkout, preview identified test processes, stop them normally, or escalate survivors:

```sh
npm run test:processes
npm run test:cleanup
npm run test:cleanup -- --force
```

These aliases use `python3 scripts/test_processes.py list|stop`; Python can be used directly without Node. Listing is read-only. Stop sends SIGTERM, waits up to five seconds, and reports survivors. `--force` adds SIGKILL for surviving processes whose identity is still verified. `--timeout 2` changes the wait per phase. `--json` produces structured output without raw command arguments or environment values. Stop exits 1 if identified processes remain; invalid scope or inspection setup exits 2. Zero means no identifiable survivors in the selected scope, not a machine-wide proof that every old test has stopped.

For future test runs, the wrapper records a private run ID and gives the command and its children an inherited marker:

```sh
npm run test:run -- npm test
npm run test:run -- npm run test:e2e
python3 scripts/test_processes.py run -- cargo test --workspace --locked
```

The wrapper returns the test command's exit code. Its private `.crewroom/test-processes` record remains after the command exits so marked detached children can still be found. Add `--run <printed-run-id>` to list or stop just that run. Existing unwrapped tests are also discovered when their arguments or recognized runtime environment paths reference this checkout's `.crewroom/test-results`. For an older fixture elsewhere, explicitly select its dedicated directory:

```sh
npm run test:processes -- --root /private/tmp/my-crewroom-fixture
npm run test:cleanup -- --root /private/tmp/my-crewroom-fixture --force
```

Repeat `--root` for multiple fixtures. Only specific subdirectories of the system temporary directory, `/tmp`, or `.crewroom/test-results` are accepted. Choose a directory reserved for disposable test work: processes referencing that directory and their observed descendants are eligible. `test:run --root <fixture> -- <command>` also records the selected fixture for children that sanitize their environment. Merely naming a binary `crewroomd`, `node`, `python`, or `tmux` never selects it.

Cleanup inspects only processes owned by the current user. Before each individual signal it rereads the process birth identity, executable and argument fingerprint. It does not signal process groups or accept arbitrary PID lists. The cleaner and its ancestors are excluded; processes referencing this checkout's `.crewroom/rust` or `.crewroom/pilot`, and their observed descendants, are protected even if marked as a test. Whole temporary roots, the checkout root and protected paths are refused. Logs, databases, worktrees, registry records and Docker containers are preserved. Container cleanup still uses the existing fixture/runner ownership receipts; this command does not prune Docker.

An already detached process that has lost both its marker and fixture paths cannot be attributed safely. An unreadable process is also not selected. Do not interpret an empty list as proof about those processes. A scrubbed descendant observed during a stop stays eligible within that invocation after its parent exits; use `--force` in that invocation when escalation may be needed. Scope older detached fixtures by their retained paths when available. The process reader supports macOS and Linux; the recorded live validation is on macOS. Run `npm run test:cleanup-tool` for disposable cleanup checks.

## Configuration inventory

The launcher requires explicit absolute data and role paths. The checkout wrapper supplies the following defaults; use explicit paths for durable installations. No entry point loads `.env` automatically.

| Native input | Default / meaning |
| --- | --- |
| Launcher `--data-dir`; checkout `CREWROOM_NATIVE_DATA_DIR` | Checkout `.crewroom/rust`; workflow, evidence and private connection descriptor |
| Launcher `--roles`; checkout `CREWROOM_ROLE_PATH` | `~/.config/crewroom/roles.json`; reusable user roles |
| Native Models & budget | `model-jobs.json` beside the role file; public selections and configuration revision |
| Native model discovery | `<runtime.root>/model-catalog/catalog.json`; private durable metadata and refresh revisions; include its process receipts in whole-state backups |
| Native Principles | `principles.json` beside the role file; reusable guidance with per-principle revisions |
| Native Playbooks | `playbooks.json` beside the role file; reusable checklists with per-playbook revisions |
| Launcher/daemon `--port` | 4318; retain the same port for live-host reattachment |
| `CREWROOM_RUNTIME_CONFIG` | Private regular JSON file owned by this user, mode 0600; absent means unavailable runner |
| `CREWROOM_TMUX` | Optional trusted tmux executable; otherwise discovered on daemon PATH |
| `<data>/jira.json` | Jira source/account metadata read by the native daemon |
| `<data>/bitbucket.json` | Repository publication/hosted-check metadata read by the native daemon |
| `<data>/crewroom.secret` | Install key: 32 random bytes, mode 0600, created once on first start. Never regenerated while encrypted values exist. See [stored credentials](#stored-credentials) |
| `<data>/credentials.json` | Jira/Bitbucket tokens saved in **Settings → Integrations**, only as `$crewroom$v1$…` encrypted values |
| `CREWROOM_JIRA_TOKEN` | Optional OAuth bearer access token for the configured Jira cloud, read server-side; overrides the saved Jira token |
| `CREWROOM_BITBUCKET_TOKEN` | Optional OAuth bearer token, read server-side; overrides the saved Bitbucket token |

Legacy `CREWROOM_DATA_DIR`, `CREWROOM_PORT`, `CREWROOM_JIRA_CONFIG` and `CREWROOM_PUBLICATION_CONFIG` do not select these native settings. Changing the selected paths opens a different installation; it does not move or import existing data. Reopening a live daemon does not reload its environment.

### Stored credentials

**Settings → Integrations** saves a Jira or Bitbucket bearer token through the local service. The service encrypts each token with XChaCha20-Poly1305 under the install key in `crewroom.secret`, binds it to its integration, and writes only the `$crewroom$v1$…` value to `credentials.json`. It decrypts tokens in memory only when it authenticates a provider request. Workspace responses, the desktop client and agent environments report only the source: `environment`, `stored`, `unreadable` or `none`. They never receive the token. A saved token takes effect on the next provider request without a restart. A non-empty `CREWROOM_*_TOKEN` in the service environment still wins and is shown as the active source.

If `crewroom.secret` is missing, is not a private (0600, owner-only) file, or does not match the stored values, the integration shows **Credentials unreadable — reconnect**. No provider request is sent. Crewroom does not create a replacement key while encrypted values exist. Restore the secret from the same backup, or use **Reconnect**. Reconnect saves the new token and drops other unreadable values; those integrations must also be reconnected.

Threat model: the install key protects tokens in copied configuration, backups, sync folders and accidentally shared files that do not also include `crewroom.secret`. It does **not** protect against processes running as the same OS user, because they can read both files. A backup that contains both files contains usable credentials.

Keep private configuration, backups and logs in operator-only directories. Runtime JSON contains provider/Mail credentials. It must not be committed or copied to the browser. A missing integration remains unavailable; saving a plan does not start an agent.

The authoritative runtime shape is [native RuntimeConfig](../crates/crewroom-runtime/src/supervisor/workflow/configuration.rs). Configure a unique absolute runtime `root`, a short private tmux `socket` directly inside that root, and an absolute `ompBinary`. For example, root `/absolute/private/runtime` can use socket `/absolute/private/runtime/tmux.sock`. On macOS the socket path must be under 104 bytes. Optional `hostPorts` must be distinct, available loopback ports; omit them for ephemeral allocation. Four ports permit one coordinator and three specialists, but concurrent research or refinement may need more.

`mail` requires a loopback HTTP URL, bearer token, project root, positive retry retention and `atcDisabled: true`. Match `retentionSeconds` to the actual service setting; 86,400 seconds is the characterized normal setting. The service must bind to loopback, require its bearer token and run with `AM_ATC_ENABLED=false`. Its database and archive are separate durable state. See [the characterized service launch](../scripts/mailbox_contract_fixture.py) for the exact v0.3.36 settings; fixture credentials and paths are disposable examples.

`models.providers` supplies each provider's API, URL, authentication mode, credential and model definitions. `catalog` supplies exact provider/model/API identities, supported effort/input values and verified tool capability. Every admitted model needs positive context/output limits in its provider model definition. Use native authentication unless the configured gateway explicitly requires `authMode: "bearer"`. Duplicate selectors and unsupported requirements fail admission. Provider credentials never enter the runner's model file: it receives an attempt-specific local gateway token.

The native **Models & budget** page writes public selections to `model-jobs.json` next to the selected `roles.json`. Each save checks its revision. **Refresh models** performs metadata discovery through the configured pinned binaries; it starts no assignments. Searchable choices show distinct routes, supported effort and blockers. Discovery requires a matching verified private `catalog` profile before a row is eligible. Failed or stale refresh retains saved selections and drafts; refresh successfully before admitting new catalog work. See the [MH4 model contract](testing/mh4-model-contract.md) for schemas, explicit equivalence, one-hour freshness, revision checks and recovery.

Roles choose Code, Judgment, Review or Fast; an optional role model overrides its job, and an optional role effort overrides the budget. Unknown or missing jobs resolve to Judgment with a warning. Bundled roles receive jobs by ID; saved explicit model/effort choices are retained. Admission captures the concrete result, so later edits leave existing assignments unchanged. Attempt history shows the frozen route and separate observed runner identity.

| Budget | Code | Judgment | Review | Fast |
| --- | --- | --- | --- | --- |
| Unlimited | xhigh | max | max | medium |
| Large | high | xhigh | xhigh | low |
| Medium | medium | high | high | minimal |
| Small | low | medium | medium | minimal |

Budget controls effort, not a spending ceiling. Unsupported model/effort combinations still block admission. The private runtime may supply `policy.jobs` (job → exact model selector) and `policy.budget` as defaults. A saved public file overrides those fields only; provider credentials, capability requirements and fallback policy remain server-side. With an older runtime configuration, `policy.default` remains effective until the operator explicitly saves job settings; the page explains this and prefills its model without silently changing effort. Use the native editor for these extended role fields; legacy browser rollback does not implement jobs.

`policy.default` supplies a model and effort for legacy configurations before that transition. `policy.fallbacks` maps each exact failed selector to an ordered list of explicit `{ model, effort }` alternatives. It handles unavailable catalog selectors before admission and retryable provider failures during execution. Runtime fallback first confirms stopped ownership, then starts a new attempt/session with a compatible, untried configured alternative. Two automatic repair/replacement cycles are available per assignment unless the operator grants more. Authorization failures, absent compatible fallbacks and exhausted cycles hold the run. History and usage retain the actual model for each attempt. The provider gateway rejects unapproved model/effort substitutions. Media, files, remote context references and unknown opaque reasoning budgets stay held. Known opaque reasoning is budgeted using the producing provider response's output count and an assignment-scoped hash ledger under `<runtime root>/budgets`.

A settled team waiting for provider-confirmed CI at the current PR head/base does not consume active execution time. Crewroom persists that wait across restart. Active agents, running tools, pending inputs and uncertain ownership prevent wait exclusion. Missing or unavailable hosted observations do not prove CI wait. Operator/review pauses exclude time only after stopped ownership is confirmed.

Provider requests have a five-minute default timeout, configurable as `models.providers.<provider>.requestTimeoutMs` from 100 to 3,600,000 milliseconds. Input-budget rejection retains the assignment. For `context_window` and `unknown_reasoning` rejections, explicit Resume starts a fresh session with the durable brief and pending input. Missing or invalid model limits, output limits and unsupported input are configuration failures: correct their settings or input before resuming; Resume may reuse the session.

Set `commandEngine.socket` to the absolute path of the selected Docker engine socket before admitting contained tool commands or repository checks. It is separate from the tmux `socket`. Docker must already be running.

Configure each repository through the workbench with an explicit path, target branch, instruction files, and required checks. A check needs an existing pinned local Docker image (`sha256:…`), absolute executable, arguments and timeout. Crewroom does not pull arbitrary images to satisfy a check. Commands run without network. Do not replace required checks with ad-hoc shell output.

Jira metadata is validated by [the native provider loader](../crates/crewroom-runtime/src/providers/config.rs): site/cloud identity, saved filter, assignee, account identity and optional allowed transition/status IDs. Bitbucket metadata is validated by [the native publication loader](../crates/crewroom-runtime/src/providers/config.rs): absolute repository path, workspace/slug/UUID, required status keys and pipeline steps. Verify account access read-only before starting a live run. The [historical pilot setup](live-pilot-setup.md) is evidence of the old account, not fresh account authorization.

## Migrations and retention

`workflow.sqlite` currently uses schema 29. Native startup upgrades schemas 1–25 to 26 in ordered per-version transactions, refuses newer schemas and refuses a nonempty unversioned store. A failing version rolls back; earlier completed versions remain committed. Back up before upgrading. Do not lower `user_version`, delete a corrupt journal, or run an old build against a migrated database. A code rollback also needs the corresponding whole-state backup.

SQLite uses WAL. Copying only the main database while it is running can lose committed work. The database alone also omits files needed to interpret it: evidence artifacts, worktrees, repository metadata, host journals, sessions, process receipts, provider reasoning budgets and Mail records.

There is no automatic run-history expiry. Recovery inputs and referenced artifacts remain retained. Completed message-update snapshots may be compacted while final events and receipts remain. Mail transport idempotency has finite retention: after expiry Crewroom reads back the actual mailbox state and holds an inconclusive outcome instead of resending blindly. Expiry never establishes delivery or completion.

Storage can grow with sessions, evidence, container workspaces and retained runs. Do not prune a live run or delete an artifact because its terminal is quiet. Export/archive and selective historical deletion are future work. Finished containers are removed only after their complete command receipts are durable. Check evidence can be recorded afterward from that retained receipt. Interrupted checks retain non-success receipts. Container logs, image identity and exit outcome remain readable after removal. Startup reconciles retained commands even if a previous controller recorded stopped ownership before it could remove them. Do not use `docker system prune` as application cleanup.

## Reusable principles

Create and edit principles in native Settings → Principles, then attach them in a role editor. Each principle has an immutable ID, name, text and revision. Saves reject stale revisions without overwriting a draft. A principle cannot be deleted while any role references it; detach it from those roles first. The assembled preview includes current principle text and becomes stale when attached principles change.

Initial admission and specialist insertion copy the referenced text and revisions into the assignment brief. Existing assignments keep their copies through edits, deletion, process replacement and restart. Missing references block new admission. A pending team proposal keeps its selected model and corresponding role job if that role's job changes before application; its principle text is copied from the current catalog when it is admitted.

### Role history and learning

Every native role create or update retains a definition in append-only history inside `roles.json`, in the same atomic file write. Imported catalogs start at the known revision; earlier versions are not reconstructed. The editor shows field differences and proposal provenance. Manual updates cannot replace history through the API.

**Ask for a change** proposes exact replacement instructions. **Teach this role** proposes appending guidance. Both record operator evidence and require a separate approval or rejection in Learning / Needs you. Approval uses the existing recoverable decision path and creates one revision. Coordinator proposals retain their recorded run evidence. Active assignments keep their captured role.

**New specialist → Draft from a description** starts one supervised Explore coordinator with a five-minute active limit. It has no repository or publication tools. A validated role proposal stops the process after it becomes idle; the operator reviews and edits the result before saving. Invalid output is retained as a tool error and creates no role. Open drafting session exposes its terminal and pause/close controls. Without a runner, the manual form remains available. Provider credentials remain in server configuration.

## Playbooks

Create and edit checklists in native Settings → Playbooks. Each has a name, a “Use when” description and one step per line. Saves check the catalog revision and retain stale drafts for review. Select an optional playbook before queueing a coding ticket, or choose **Change playbook** in a delivery/research session and enter a reason. Selecting a new playbook resets its copied steps to pending and records a new selection version. Catalog edits leave existing run copies unchanged.

At run start the coordinator calls `playbook_status` and may use `propose_playbook` to suggest a catalog entry with a rationale. The session header presents the proposal for operator selection. A proposal alone changes no checklist. Selecting it copies the proposed catalog revision. Changes after admission also create a durable steering message; a paused run shows pending delivery until Resume. Request identities prevent duplicate messages on retry.

Only the current active coordinator can call `record_playbook_step`. It supplies the selection version, step ID and state (`pending`, `active`, `done` or `skipped`); a skipped step requires a reason. Stale versions are rejected. The checklist, reasons and operation receipts survive restart. Progress is advisory: it creates no check/review evidence, advances no workflow stage and never grants readiness. Explore, Grill and specialist drafting have no playbook tools.

## Backup and same-path restore

1. Pause all admitted work and verify stopped ownership. Resolve any unknown surviving child before treating a backup as consistent. Stop Crewroom and then its separately managed Mail service. Stop editing the configured repositories during capture.
2. Record the application commit, runtime versions, absolute paths and repository identities. Copy the complete Crewroom data directory, runtime root, role file and adjacent `model-jobs.json`, `principles.json` and `playbooks.json`, private integration metadata (the data directory's `crewroom.secret` **with** `credentials.json`; neither is usable without the other), Mail database **and** archive, and the configured repositories with their Git common directories. Keep any remaining SQLite `-wal`/`-shm` files with their database. Preserve permissions. Keep a private copy of required pinned image identities and a reproducible way to obtain those exact local images.
3. Verify a file/hash inventory of the backup. Store it outside the live paths with operator-only access. Credentials and ticket/repository content may be present; keep it out of source control and public artifacts.
4. To restore, first stop all relevant services. Preserve the failed state separately for diagnosis. Restore every captured component to its original absolute path with the matching application/runtime versions. Do not start a second controller against a copied database. Linked worktree `.git` files refer to the original Git common directory; moving the backup to arbitrary new paths is not a supported restore.
5. Start Mail with its retained state, then Crewroom. Inspect recovery holds, requirements, queue, steering, message outcomes, candidate revisions and evidence. Confirm owned worktree identity and unchanged user checkouts. Reconciliation may require attention; a restored snapshot must not silently restart uncertain external writes. Resume only the explicitly selected paused run after its ownership/evidence checks pass.

The earlier native operations fixtures verified complete stopped inventories (693 entries for the component build; 697 for the then-integrated bundle). The [final signed-package headless receipt](testing/rust-native-release-headless-2026-10-04.json) now verifies a 693-entry whole-state restore and subsequent research on candidate `8f3e325870e40209f42c43ef0009e953f45398f5`, plus launch/reopen and three actual runtime profiles. These checks use scripted providers. The [separate visible-GUI receipt](testing/rust-native-final-gui-2026-10-04.json) records interaction, close/reopen and bounded resource samples. The repository-default switch and unverified optional VoiceOver/OS input-method compatibility are recorded in the [native release record](release-native.md).

For code rollback, retain the **matching old build plus stopped pre-cutover snapshot**. The separate rollback fixture rebuilt accepted M7 commit `965a563b6a42dcc5b4511927585644fe08f4d023`, inventoried 7,793 old-build entries and captured 58 installation entries containing two unadmitted queued plans and pending steering. Native startup preserved them; after native edits, 62 newer-state entries were preserved separately. Restoring the original snapshot at its original paths let the unchanged old build recover the original facts and save new work. This proves bounded unadmitted legacy rollback, not adoption of active legacy processes or old-code compatibility with newer state. Full native runtime restoration is the complementary separate proof.

Historical data can retain conservative native ownership or checked-promotion holds. Preserve journals, worktrees and evidence when proof is missing. Use the documented recovery action only when its required identity/receipt proof exists; no automatic database repair is provided. See [legacy holds](native-installation.md#conservative-legacy-holds).

Before reconnecting integrations after rollback, reconcile remote actions since the snapshot against the separately preserved newer receipts and provider readback. A restored snapshot must not silently repeat an external write.

## Failed-service recovery

| Failure | Operator action and required evidence |
| --- | --- |
| Crewroom daemon exits | Restart the same build/configuration/data paths. Inspect the recovered processes and holds. Do not start a duplicate plan. |
| Agent or host exits | Let bounded application recovery reconcile the existing assignment/session. At exhausted cycles, inspect the retained reason; an extra allowance requires an explicit operator action. |
| Mail is unavailable | Restore the same Mail database/archive/configuration. Retain pending sends; reconcile existing keys. Unknown expired sends require inspection. |
| Docker is unavailable | Restore access to the configured engine/socket. Until owned command termination and receipts can be read, keep the run held. |
| Provider credentials/model fail | Correct server-side credentials or supported configuration and restart as needed. Inspect the recorded assignment and failure before resuming. Do not substitute a hidden model. |
| Jira/Bitbucket loses a write reply | Use the existing reconciliation path. An inconclusive readback stays pending/unknown; creating another PR/comment is not recovery. |
| Git child or worktree ownership is uncertain | Preserve paths, journals, identity records and edits. Use the existing operator recovery decision; matching HEAD alone does not prove the child stopped. |

Cleanup must verify Crewroom ownership for each exact PID, path or container. PID reuse, missing receipts, changed labels and symlinked paths are reasons to retain a resource for inspection. Never kill a process by name, delete a user checkout, or remove all containers sharing a name prefix.

### Pair checkpoint and recovery

New coding admission starts the coordinator, which binds the accepted plan to a versioned staged choreography. Pair exchanges return pending intents; the Mail ledger records external storage and runner receipt separately. Checkpoint transfers stop and reconcile the run before releasing the writer. A named successor has no write authority until admitted and claimed. Documentation completes before final candidate freeze.

After adopting new requirements, Resume leaves the old writer unclaimed. Reconcile and retire the previous stage, accept a plan for the adopted brief, and propose fresh assignments for every affected package. Prior plans, reports, snapshots and exchanges remain historical. Candidate changes need new checks and independent review; PR observations still require explicit Resume before work.

New container command intents pin the desktop user’s numeric UID/GID. This lets capability-dropped checks read private candidate exports without relaxing file modes. The daemon verifies that identity during Docker reconciliation and includes it in new command/check receipts. Existing receipts retain their captured environment. Images must support execution as that numeric user; writable temporary files belong under `/tmp`, and read-only worktree mounts stay protected.

Native Codex/Claude tool failures retain the conservative MH2/MH3 hold: even a stale choreography revision rejected without a package mutation can leave an unresolved native receipt. Reloading a revision does not reconcile that held attempt. Preserve its journal, source and ownership proof; do not rewrite a result or treat a fresh job as recovery of the held session. Generic reconciliation of definite rejected tool calls remains outside this milestone.
