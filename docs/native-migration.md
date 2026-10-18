# Rust and GPUI migration

The operator requested the native migration on 2026-10-02 and selected **both the UI and workflow service** for the Rust port. [ADR 0009](adr/0009-rust-native-workbench-and-service.md) records that decision. The accepted product behavior remains the target. This is a staged port, not a feature reduction or a claim that a native window completes the work.

## Current acceptance

The full Rust/GPUI migration is accepted on `codex/rust-gpui` within the supported Apple Silicon macOS environment. The [native release record](release-native.md) binds the original signed candidate, native workflow and interaction evidence, optional unverified OS accessibility/input compatibility, installation/recovery and final repository-default cutover. The [acceptance reconciliation](testing/rust-native-acceptance-reconciliation-2026-10-04.json) distinguishes direct executions from composed evidence; scripted providers are not new live delivery.

`npm run dev` and `npm start` now invoke the debug/release native checkout launcher. The browser implementation remains available through explicit `dev:legacy` and `start:legacy` commands for compatibility and matching-build rollback. No active installation, role catalog or retained pilot was moved or restarted by changing repository defaults.

## Build and run

Follow [native installation](native-installation.md) for prerequisites and packaging and [operations](operations.md#start-and-stop) for explicit private data/role paths. The checkout command is:

```sh
./scripts/run-native.sh --release
```

The script builds locked dependencies into a new immutable ad-hoc signed bundle and invokes its adjacent nonbuilding launcher. Installed use needs no checkout, Node, Rust or Zig:

```sh
/path/Crewroom.app/Contents/MacOS/crewroom-launcher \
  --data-dir /absolute/private/data \
  --roles /absolute/private/config/roles.json
```

The independent daemon survives GUI closure. Reopening reuses its authenticated connection and existing configuration; a new service binary or changed runtime configuration requires an explicit stopped restart. The checkout defaults remain `.crewroom/rust`, loopback port 4318 and `~/.config/crewroom/roles.json`. Use `CREWROOM_NATIVE_DATA_DIR` and `CREWROOM_ROLE_PATH` to select a separate installation. The browser store and pilot are never imported automatically.

## Implemented boundary

| Crate | Current responsibility |
| --- | --- |
| `crewroom-core` | Schema-26 SQLite records; independent source/refinement fields; accepted history; local/Jira snapshot intake; repository selection; queued plans and reorder/removal; peer/operator/controller delivery records; local dependencies; refinement/research plans; explicit requirements adoption; user roles; immutable assignments and attempts; bounded repair/fallback receipts; event/observer records; captured launch and slot scheduling; queued review/feedback Resume; team reservations, proposals and retirement/admission; evidence-backed learning and catalog recovery; refinement/research outcomes; coding/publication/Jira records; cancelled successors and ownership handoff; guarded repository edits and complete snapshot projections; branch reservation, writer/repair accounting and target/conflict recovery records; active-time accounting; lifecycle rules; controller lock |
| `crewroom-runtime` | Separate Rust process helper; epoch-fenced checked execution, immutable manifests/seals, bounded output and verified process shutdown; OMP v2 RPC, exact model preflight and observation/pane projections; provider gateway, usage and durable input budgets; contained Docker commands and retained receipts; complete tool catalog and durable host input/event journals; complete independent OMP/gateway/container host integration; external Mail transport, durable message delivery and passive observation; managed Git/worktree ownership, writer grants and scoped source tools; full workflow supervision, authenticated tools, handoffs and interruption recovery |
| `crewroom-service` | Separate `crewroomd` authority; configured workflow/Git/evidence/Mail/Jira/publication composition; durable coding scheduler; own-origin operator HTTP and separate host authentication; workspace/run/result/event WebSocket updates; bounded viewers; recovery and drained SIGINT/SIGTERM/drop shutdown |
| `crewroom-desktop` | GPUI Kit workbench; intake/refinement/research, repository/role/model forms, launch/queue/dependencies, steering/insertion/learning, lifecycle/continuation, saved evidence/history, read-only Ghostty wall and transcript, keyboard/accessibility/input, drafts and reconnect |

The GUI writes through the service. SQLite and Git preflight run on a blocking worker, outside the async HTTP event loop. The Rust controller shares the TypeScript controller's OS lock paths and holds exclusive execution authority through recovery and shutdown. Either runtime refuses a workspace owned by the other.

Rust opens fresh databases and upgrades schema versions 1–25 in ordered transactions to schema 26. It refuses newer, unversioned nonempty and symlinked databases. Back up the complete stopped installation before opening older state; the [historical upgrade receipt](testing/rust-native-upgrades-2026-10-03.md) records supported upgrades and conservative legacy holds. Interoperability currently covers tickets/imports/refinements/history/notes, queue/steering/reorder/removal receipts, repository create/retire receipts and the controller lock. Repository replay after checkout removal uses the saved canonical path. N2.1 adds dependency decisions, alternate plans, and requirements-adoption transactions. Its operation-sequence oracle compares durable records against the TypeScript implementation, including retained assignment/repository/publication fixture rows. It does **not** establish recovery of an executing legacy run.

The N2.2–N2.6 controller-owned store APIs and development-only `ledger-oracle` example do not expose process-authority mutations over HTTP. Their clock-controlled tests reopen the store for every native operation, compare all 25 tables, and preserve exact receipt timestamps.

The configured native service now admits coding, research, Explore and Grill workflows and exposes the complete nonterminal operator API. Execution requires `CREWROOM_RUNTIME_CONFIG` to name a regular, nonsymlinked JSON file owned by the current user with private permissions (mode 0600); it uses the validated runtime/model/Mail configuration contract. The adjacent `crewroom-process` and `crewroom-host` binaries are required. `CREWROOM_TMUX` overrides PATH lookup. Jira and Bitbucket use private `jira.json` and `bitbucket.json` files in the selected data directory, as described in the provider configuration checkpoints below. Keep the same daemon port across restart to reattach trusted live host endpoints; changed origins are reconciled by stopping those retained endpoints. The operator descriptor is published only after startup recovery completes.

Missing runner/provider configuration remains unavailable. Queue admission is distinct from execution, and stored steering remains pending until external delivery/host receipt. The native workbench exposes the accepted controls, live views and read-only terminal wall. Browser compatibility startup is explicit through `npm run dev:legacy` or `npm run start:legacy` with separate legacy state.

## Ordered migration and acceptance

The [execution ledger](native-migration-tasks.md) records the completed stage order and the operator's commit-and-push requirement. The stage descriptions and checkpoint contracts below retain their historical sequencing; use current acceptance above and [status](status.md) for the final disposition. Preserve revision-bound evidence and the retained M7 fixtures as contract oracles.

1. **N1 — Native shell and durable intake.** Build/run the separate Rust service and GPUI app. Preserve schema-26 intake, optimistic refinements, history, repository selection, queued plans, pending steering and role storage. Prove lock exclusion, token/origin checks, push updates, reopen, and UI-close/service independence. See the [progress record](testing/mvp-progress.md#n1--rust-native-intake-and-service-foundation-2026-10-02).
2. **N2 — Schema-26 durable workflow parity (accepted within recorded boundaries).** N2.1 implements dependency decisions/projections, Explore/Grill/research plans, read-only research pinning, explicit requirements adoption, safe refinement closure and shared run-save effects. See the [progress record](testing/mvp-progress.md#n21--durable-dependencies-plans-and-requirements-adoption-2026-10-02) for its bounded evidence. N2.2 adds assignment/attempt, observation and active-time transactions; its [143-operation oracle and regression gates](testing/mvp-progress.md#n22--assignment-attempt-and-observation-ledger-2026-10-02) pass. N2.3 adds captured launch, slot scheduling, control/replay and feedback-continuation records; its [133-operation oracle and regression evidence](testing/mvp-progress.md#n23--captured-launch-scheduling-and-feedback-continuation-2026-10-02) passes with a documented unchanged domain retry. N2.4a adds peer/operator/controller message transactions and stopped-input restoration; its [122-operation oracle and regression evidence](testing/mvp-progress.md#n24a--durable-message-delivery-records-2026-10-02) passes. N2.4b adds team reservations, proposals, retirement/admission and transition records; its [142-operation oracle and regression gates](testing/mvp-progress.md#n24b--team-proposals-insertion-and-retirement-2026-10-02) pass. N2.4c adds versioned learning, operator decisions and catalog recovery; its [76-operation oracle and regression gates](testing/mvp-progress.md#n24c--learning-decisions-and-catalog-recovery-2026-10-02) pass. N2.5a adds refinement decisions and research outcome records; its [95-operation oracle and regression gates](testing/mvp-progress.md#n25a--refinement-and-research-outcomes-2026-10-02) pass. N2.5b adds coding/publication records, stale-feedback retention and Jira synchronization; its [86-operation oracle and regression gates](testing/mvp-progress.md#n25b--coding-publication-and-jira-records-2026-10-02) pass. N2.5c adds cancelled continuation, atomic handoff, repository edits and complete snapshot projections; its [119-operation oracle and regression gates](testing/mvp-progress.md#n25c--cancelled-continuation-repository-edits-and-projections-2026-10-02) pass. N2.6 adds repository ownership and target-recovery transactions; its [166-operation oracle and regression gates](testing/mvp-progress.md#n26--repository-ownership-and-target-recovery-records-2026-10-02) pass. N2.7a input/clock checks pass with [804 boundary comparisons](testing/mvp-progress.md#n27a--request-validation-and-exact-clocks-2026-10-02). N2.7b replay/preflight checks pass with [39 comparisons and responsive actual Git tests](testing/mvp-progress.md#n27b--serialized-replay-and-responsive-git-preflight-2026-10-02). N2.7c [source-date, JSON and 154-operation closure](testing/mvp-progress.md#n27c--source-dates-serialized-identities-and-store-closure-2026-10-02) passes. Historical schema upgrades remain N7.1 and controller change notifications remain N3.3. Compare operation sequences and reopen behavior with the existing tests.
3. **N3 — Rust supervision and external messages.** N3.1a [process authority and checked execution](testing/mvp-progress.md#n31a--native-process-authority-and-checked-execution-2026-10-02) passes 25 real-process scenarios and the full gates; service dispatch remains fenced. N3.1b1 [OMP transport/model preflight](testing/mvp-progress.md#n31b1--native-omp-transport-model-preflight-and-observations-2026-10-02) passes 12 guarded runner scenarios, 621 policy and 12,590 observation/pane comparisons. N3.2a [provider gateway/usage/input budgets](testing/mvp-progress.md#n32a--native-provider-gateway-usage-and-input-budgets-2026-10-02) passes 1,568 protocol comparisons, nine direct HTTP tests, 21 compression cases and four guarded OMP gateway scenarios. N3.2b [contained commands](testing/mvp-progress.md#n32b--native-contained-commands-and-receipt-preserving-cleanup-2026-10-03) passes eight real-Docker scenarios and 14 container-boundary tests. N3.1b2a [durable host protocol](testing/mvp-progress.md#n31b2a--durable-host-protocol-and-tool-catalog-2026-10-03) passes 2,332 comparisons and three journal-process scenarios. N3.1b2b [independent host integration](testing/mvp-progress.md#n31b2b--independent-native-host-integration-2026-10-03) passes 16 control/process cases, four real Docker host scenarios and 11 guarded OMP/tmux cases. N3.3a [external Mail and durable delivery](testing/mvp-progress.md#n33a--native-external-mail-and-durable-delivery-2026-10-03) passes eight transport, ten messaging and five real Mail kill-window scenarios. Continue repository/evidence/provider dependencies and complete supervision. Reuse actual OMP/tmux/Mail/Docker fault harnesses with scripted providers. Prove interruption and descendant containment before admitting coding.
4. **N4 — Repository and revision evidence.** Port managed worktrees, writer leases, scoped tools, target/conflict recovery, actual checks, independent review and content-hashed artifacts. Re-run target movement, interrupted promotion, stale requirements, forged evidence and same-worktree continuation cases.
5. **N5 — Delivery parity.** Port Jira synchronization/write reconciliation, Bitbucket push/PR/checks/feedback, unknown-action recovery, provider-observed merge dependencies and same-PR continuation. Pass synthetic delivery errors before new live work. Do not repeat the retained merged pilot.
6. **N6 — Complete native operator flows.** Add remaining configuration/learning/refinement/research controls, evidence/diff/artifact views, usage/history, and the read-only Ghostty wall using the tested lab. Validate overlays, keyboard focus, selection/copy, scrollback, reconnect and sustained output. VoiceOver and advanced OS input-method validation are optional unverified follow-ups after the 2026-10-04 scope clarification. Add native equivalents for all 31 browser journeys.
7. **N7 — Cutover and release.** Require complete contract coverage, real-process fault checks, whole-state restore, fresh install/repeat use, measured production footprint and packaging. Then make Rust the default and retire the browser implementation. Distribution signing/notarization, cross-platform support and release performance remain unverified.

## Checks

```sh
cargo test --workspace --locked
cargo build --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
PATH=/opt/homebrew/opt/node/bin:$PATH node_modules/.bin/tsx scripts/rust/verify-compatibility.ts
PATH=/opt/homebrew/opt/node/bin:$PATH node_modules/.bin/tsx scripts/rust/verify-workflow.ts
cargo build --locked -p crewroom-core --examples
PATH=/opt/homebrew/opt/node/bin:$PATH node_modules/.bin/tsx scripts/rust/verify-attempts.ts
PATH=/opt/homebrew/opt/node/bin:$PATH node_modules/.bin/tsx scripts/rust/verify-scheduling.ts
PATH=/opt/homebrew/opt/node/bin:$PATH node_modules/.bin/tsx scripts/rust/verify-messages.ts
PATH=/opt/homebrew/opt/node/bin:$PATH node_modules/.bin/tsx scripts/rust/verify-teams.ts
PATH=/opt/homebrew/opt/node/bin:$PATH node_modules/.bin/tsx scripts/rust/verify-learning.ts
PATH=/opt/homebrew/opt/node/bin:$PATH node_modules/.bin/tsx scripts/rust/verify-outcomes.ts
PATH=/opt/homebrew/opt/node/bin:$PATH node_modules/.bin/tsx scripts/rust/verify-providers.ts
PATH=/opt/homebrew/opt/node/bin:$PATH node_modules/.bin/tsx scripts/rust/verify-continuations.ts
PATH=/opt/homebrew/opt/node/bin:$PATH node_modules/.bin/tsx scripts/rust/verify-repository-work.ts
PATH=/opt/homebrew/opt/node/bin:$PATH node_modules/.bin/tsx scripts/rust/verify-boundaries.ts
PATH=/opt/homebrew/opt/node/bin:$PATH node_modules/.bin/tsx scripts/rust/verify-replay.ts
PATH=/opt/homebrew/opt/node/bin:$PATH node_modules/.bin/tsx scripts/rust/verify-dates.ts
PATH=/opt/homebrew/opt/node/bin:$PATH node_modules/.bin/tsx scripts/rust/verify-serialization.ts
PATH=/opt/homebrew/opt/node/bin:$PATH node_modules/.bin/tsx scripts/rust/verify-closure.ts
PATH=/opt/homebrew/opt/node/bin:$PATH node_modules/.bin/tsx scripts/rust/verify-processes.ts
```

The lifecycle fixture has 4,536 expected transitions generated by the TypeScript implementation at baseline `e91d25894b3eae8890344fa87ef5682557f87b1d`. `scripts/rust/lifecycle-corpus.ts` regenerates the compressed oracle; `export-schema.ts` and `export-roles.ts` record the source schema/catalog. These are development checks, not runtime dependencies. The Rust binaries do not launch Node or evaluate TypeScript.

Continue `npm test`, `npm run build` and `npm run test:e2e` while the old release remains supported. Those checks establish retained-release behavior, not native execution or native UI parity.

> The following checkpoint notes retain their historical acceptance boundaries. Statements about unadmitted execution or pending integration describe that checkpoint; the current acceptance section above governs today’s daemon capabilities.

## Historical implementation checkpoints

These entries describe each bounded checkpoint when it landed. Later integration supersedes their then-pending dependencies; they do not reopen completed migration work.

## N2.1 service operations

Authenticated POST operations now include:

| Path | Durable result |
| --- | --- |
| `/api/tasks/:id/dependencies` | Replace local dependencies with a keyed optimistic decision; reject duplicates, missing tickets and cycles |
| `/api/tasks/:id/refinement-runs` | Queue one Explore or Grill plan without requiring an accepted coding brief |
| `/api/tasks/:id/research-runs` | Queue research against the accepted brief; pin a selected repository's target commit without modifying its checkout |
| `/api/runs/:id/adopt-requirements` | Explicitly adopt the latest accepted requirements; retain old assignment briefs and flag changed criteria |
| `/api/runs/:id/close` | Close an Explore/Grill run only when its attempts are unclaimed or confirmed stopped |

These operations share the native token/origin boundary and push notifications. Adoption of retained active-state fixtures requests pausing, creates pending steering, rejects outdated operator insertions and changes the repository requirements hash atomically. It cannot start, stop or reconcile a real host. Publication receipts in the dependency tests are fixtures; an open or ready PR does not count as a merge. No scheduler or execution capability is enabled by this slice.

## N2.3 captured launch API

`POST /api/tasks/:id/runs` accepts an optional normalized launch. `POST /api/queue/:id/launch` saves a keyed launch update after checking the enabled role catalog and accepted brief. A saved launch is still a queued plan; these endpoints admit no assignments and execute no processes. Slot and recovery mutations remain controller-owned store APIs until the real scheduler passes N3/N4.

## N2.4c learning decisions

`POST /api/learning/:id/decision` accepts a keyed operator approval or rejection for a saved proposal. Role approvals persist before the catalog save and reconcile on daemon startup; project guidance updates atomically with the decision. The API uses the same authenticated loopback/origin boundary as other mutations. Proposal creation remains a controller-owned method until N3 coordinator authentication is validated. The native learning editor remains N6 work.

## N2.5a refinement decisions

`POST /api/refinement-proposals/:id/decision` accepts a keyed operator acceptance or discard. Acceptance may edit the proposed brief/criteria and records provenance in the accepted requirements; existing run snapshots change only through explicit adoption. Proposal creation and research artifact/completion writes remain controller-owned store methods until native supervision and artifact verification pass. Native editor controls remain N6 work.

## N2.5c repository edits and continuation records

`POST /api/repositories/:id` performs asynchronous Git preflight, rechecks configuration/reference guards and retains a keyed edit receipt. Replay succeeds after service restart even if the checkout later moves. Captured repository snapshots, readiness holds and cancelled-run continuation reasons are now projected. Continuation queue/transfer methods remain controller-only; `POST /api/runs/:id/continue-later` refuses execution until actual ownership/head/provider verification is ported.

## N2.7b creation and research preflight

Repository creation and research Git checks run outside the workflow mutex. Workspace reads and other mutations remain available during those checks; research admission rechecks the current task and repository in its transaction. Creation and research receipts replay before touching Git, including after service restart and checkout movement when the request path matches the saved canonical path. A changed request identity refuses replay. These routes still queue plans only; native runner admission remains unavailable.


## N3.1b2b independent host contract

`crewroom-host /absolute/private/host.json` is the controller-owned native adapter. It requires a fresh canonical private attempt root, distinct control/provider tokens, a fixed loopback backend origin, exact model/provider configuration, an absolute runner command and an allowlisted environment. The runner receives isolated model/settings/MCP files and a private TMPDIR. The endpoint is published only after model, tools and session identity are confirmed. The native daemon does not yet launch it.

Input admission, runner receipt, terminal disposition and backend storage are separate facts. Event pages are non-consuming; pane text is bounded observation. Close drains registered input/tool work and verifies actual OMP/container ownership; a missing engine or unresolved child keeps shutdown pending. An interrupted backend call still needs controller-side effect reconciliation. Real OMP history restoration requires the same assignment working directory; unusable history opens the new attempt's session directory explicitly. Native N4 check services must locate command receipts at `tools/SHA256(toolCallId)` below the attempt root.


## N3.3a external Mail contract

The runtime Mail client uses a fixed authenticated loopback endpoint, shared initialization and bounded requests. It keeps ambiguous mutations pending; keyed send reconciliation confirms an existing receipt or one exact inbox match after expiry. Registration binds durable names first and acceptance saves input before acknowledging externally.

Passive event observation uses durable whole-page cursor transactions, a 1.5-second throttle, one active poll, round-robin mailbox paging and a twelve-second total deadline. Each poll stops at 100 pages or after reaching 1000 events or eight MiB of retained events, checked after saving a complete page. The final wire-bounded page may cross the event or byte threshold. Native daemon dispatch remains unavailable until repository/evidence/provider services and complete supervision pass.


## N4.1 repository contract

The native repository boundary reserves an isolated worktree before dispatch, persists checked Git operation identity and proves quiescence before adopting interrupted work. Current ownership, requirements and coordinator grants are rechecked under the same store guard as dispatch or the durable receipt. A stopped child is not a successful Git result.

Scoped tools preserve separate raw source manifests and replacement-decoded text hashes. Reads, literal/regex search, pinned revision/research reads and durable whole-file/exact-span edits enforce the recorded repository boundary. The configured ad-hoc Docker spec mounts the worktree read-only and does not produce check evidence. [N4.1 evidence](testing/mvp-progress.md#n41--native-repository-ownership-and-scoped-work-2026-10-03) covers 19 scenarios, three killed-process windows and one real contained command.

Next complete N4.2a revision/check/review/research evidence and N4.2b target recovery/retained-work proofs. Native service admission remains fenced through N5 delivery and full N3.3b supervision. Conservative UTF-8 path/instruction and noncanonical legacy-pointer refusals are recorded for the N7 history audit.


## N4.2a evidence contract

Native ordinary completion, exact candidate lineage, private check snapshots, actual Docker receipts, immutable artifacts, independent review/readiness and pinned research proof now have [revision-bound evidence](testing/mvp-progress.md#n42a--native-revision-and-research-evidence-2026-10-03). Candidate/feedback/completion receipts commit atomically. Actual check evidence persists before container cleanup; unavailable cleanup is retained and retried without duplicating the check. Missing observations block readiness without being reported as failed execution.

The native workflow remains unadmitted. Complete target/conflict recovery and retained-work verification (N4.2b), configured Jira/Bitbucket delivery (N5), full supervision (N3.3b), native operator journeys/terminal and installation/restore/cutover (N6–N7).


## N4.2b1 target refresh contract

Native target refresh, rollback and isolated conflict preparation now have [revision-bound evidence](testing/mvp-progress.md#n42b1--native-target-refresh-and-rollback-2026-10-03). Every mutable Git child has a durable checked descriptor. A numeric live exit requires complete persisted output; stopped ownership alone never proves success. Source, accepted scope and actual target observations are rechecked before checkpoint acceptance. A refreshed candidate and its target association commit atomically.

Preparation alone does not admit a conflict writer. Complete scoped conflict continuation, checked promotion and retained-work transfer (N4.2b2), configured providers (N5), full supervision (N3.3b) and native operator/terminal/installation/cutover (N6–N7) before migration acceptance.


## N4.2b2a scoped conflict contract

Native conflict tools and keyed checked continuation have [revision-bound evidence](testing/mvp-progress.md#n42b2a--native-scoped-conflicts-and-continuation-2026-10-03). The original branch remains unchanged during resolution. Separate worktree identity, accepted instructions, source candidate and pinned target constrain reads, writer admission/restoration and recorded-path edits. Ready closes editing. A durable continuation precedes checked staging and rebase; unknown outcomes and pause cannot become ready receipts.

The checked helper now supplies a bounded ephemeral exit report only after capture durability succeeds. Its live parent requires the report to match the observed helper exit. This report is never persisted or used as restart proof; recovery still checks actual Git effects and stopped ownership. Complete conflict promotion/evidence (N4.2b2b), cancelled retained-work transfer (N4.2b2c), providers, full supervision and native operator/release gates before accepting the migration.


## N4.2b2b promotion and cleanup contract

Native conflict completion and checked promotion have [revision-bound evidence](testing/mvp-progress.md#n42b2b--native-conflict-completion-and-promotion-2026-10-03). A persisted reset is never relaunched. Stopped ownership and exact physical source determine recovery; launch admission remains paused and stopped even when existing effects can be reconciled from an attention state.

Source/resolver candidate lineage, completion, refresh association and target notice commit atomically before temporary workspace cleanup. Cleanup preserves external data and uncertain children, checks the exact branch value, and tolerates a historical cwd disappearing only for immutable descriptor comparison. Current launch paths, saved manifest hashes and owner/lease proofs remain required. Retained-work transfer, provider delivery, full supervision and native operator/release gates remain outstanding.


## N4.2b2c retained-work contract

Native cancelled-source verification and successor transfer have [revision-bound evidence](testing/mvp-progress.md#n42b2c--native-cancelled-retained-work-transfer-2026-10-03). Keyed queue replay precedes external checks. Admission requires exact retained source, stopped historical Git ownership and fresh publication verification; it transfers branch/worktree ownership atomically without copying predecessor validation.

Prelaunch replay revalidates the original source. Repeated successors retain publication lineage. Changed accepted requirements belong to the successor, while a moved target preserves the old base until explicit refresh. Dependency merges must be present in that accepted base. Configured provider observations, full supervision and native operator/release gates remain required before native workflow admission.


## N5.1 provider client contract

Native Jira/Bitbucket client and private credential configuration have [revision-bound evidence](testing/mvp-progress.md#n51--native-provider-clients-and-private-configuration-2026-10-03). Production origins are fixed; credentials stay private, redirects and retries are disabled, and the dispatch callback precedes the conservative sent boundary. Requests have a 15-second response/body deadline and a 4MiB decoded limit. Pagination and resource identities are validated before complete results are returned; comment failures retain valid earlier entries as partial observations.

Credential renewal/cache is shared and continues when one waiter is cancelled. Bounded command output and errors do not expose token values. All credential/provider evidence uses synthetic values and local fixtures. Native JSON parsing retains serde_json nesting limits and refuses escaped unpaired UTF16 surrogates; tests distinguish these stricter refusals from Node behavior and retain unknown mutation outcomes. Both transports reject more than five content codings.

This is a library boundary. Checked publication Git (N5.2), durable delivery and observer integration (N5.3), full supervision and the native operator/release gates remain required. Native workflow admission stays fenced.


## N5.2 publication Git contract

Native checked publication Git has [revision-bound evidence](testing/mvp-progress.md#n52--native-checked-publication-git-2026-10-03). Each command directory has one OS lease. The caller's final synchronous authorization encloses durable intent, checked prepare and actual launch after recovery. Cancellation does not abandon registered workers; close stops verified children and drains workers before releasing the lease. Recovery validates all retained intents, seals unlaunched commands and never replays old writes. Unfamiliar legacy roots or uncertain identity remain held.

Commands use strict branches/hashes, protected targets, explicit expected-head leases, request-owned target refs, bounded environment and private diagnostics. A live numeric helper report plus matching exit and complete captures is required. Deadlines, missing outcomes and overflow stay unknown. The helper's per-stream truncation marker only indicates overflow; it is never exit or success proof. Local fixture tests exercise actual Git and checked OS processes, including four killed controllers; no real provider or credential is used.

N5.3 owns durable request replay before target fetch, remote readback after writes and provider workflow integration. Full supervision, native operator journeys, legacy upgrade and cutover remain required. Native workflow admission stays fenced.


## N5.3a Jira workflow contract

Native Jira synchronization and durable reporting have [revision-bound evidence](testing/mvp-progress.md#n53a--native-jira-synchronization-and-durable-reporting-2026-10-03). Complete search and retained-issue reads precede atomic import. Reporter requests retain keyed intent, separate sending/unknown/confirmed state and ordered lanes. Exact remote readback reconciles unknown effects without duplicate writes. Review and close derive from provider receipts; research summaries require current verified completion.

Evidence-derived content is bound to relevant durable facts across async assessment and credential waits. Final authorization prevents stale writes after controls or evidence change. Registered refresh/flush tasks survive cancelled waiters; close fences admission and drains them. Local fixture evidence includes actual controller kills, Git, Docker, artifacts and SQLite failures, with synthetic HTTP/credentials and explicitly seeded agent/publication records.

These libraries are not yet wired into native daemon execution. Complete N5.3b publication/target/observer integration, N3.3b supervision and N6-N7 before native admission and release cutover.


## N5.3b1 remote and target contract

Native fresh-target and cancelled-PR observers have [revision-bound evidence](testing/mvp-progress.md#n53b1--native-fresh-targets-and-provider-observers-2026-10-03). Configured repository identity is preserved separately from normalized HTTP payload schemas. Weak read-only repository callbacks observe live remote facts without acquiring repository operation locks.

Remote operations register before await, outlive cancelled waiters and drain at Stop/close. Per-run epochs prevent an older queued operation from launching after Stop. One checked Git owner uses each command directory; unknown cleanup is retained as an error. Target fetch authorization is rechecked synchronously at launch. Request replay reads its durable receipt before provider access, and ordinary candidate/receipt/notice association commits together.

Complete N5.3b2 publication/hosted-evidence integration and N3.3b supervision before native admission. Existing historical split associations and plain process journals require the N7 audit. N6-N7 operator/terminal, installation and cutover gates remain open.


## N5.3b2 publication contract

Native publication has [revision-bound evidence](testing/mvp-progress.md#n53b2--native-publication-and-hosted-evidence-2026-10-03) for durable intents, checked push, exact PR readback, hosted readiness, feedback, merged dependencies and freshly validated same-PR successors. Provider HTTP and credentials are synthetic; Git, checked processes, Docker checks, SQLite and local evidence artifacts are real.

Publication writes recheck current evidence and control after asynchronous work. Shared flushes and prepares cannot overwrite newer publication data, and each pending run is registered before the first provider wait. Stop fences older pending work; shutdown also drains already attempted effects and verified Git cleanup. Unknown PR creation is reconciled by identity before any new POST. Latest description and identity observations participate in readiness, while observation outages alone do not revoke a prior review.

The daemon execution fence remains until full N3.3b supervision, scheduling, internal host authentication and startup/stop integration pass their actual OMP/tmux/Mail/Docker matrices. N6 operator and terminal work, accessibility, N7 installation/history/restore and final release cutover remain outstanding.


## N3.3b1 supervised process contract

Native supervised launch, passive observation, same-process reattachment and verified cleanup have [revision-bound evidence](testing/mvp-progress.md#n33b1--native-supervised-process-lifecycle-2026-10-03). Driven work survives cancelled callers. Slow observations preserve live ownership; uncertain identities block recovery. Stop fences queued launches and preserves container receipts, while malformed journals and failed store receipts cannot skip other owned-process cleanup.

Four guarded actual-OMP cases exercise the new lifecycle with synthetic provider/backend replies, and 17 focused scenarios cover real host/tmux/process and Docker failures. Complete workflow composition and the integrated OMP/tmux/Mail/Docker matrix remain N3.3b2/b3. The daemon still refuses native agent admission; N6–N7 remain required.


N3.3b3 production composition and scheduler acceptance is recorded in [the current progress entry](testing/mvp-progress.md#n33b3--native-daemon-and-scheduling-2026-10-03). N6–N7 remain open.
