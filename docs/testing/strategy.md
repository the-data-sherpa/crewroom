# Crewroom offline test strategy

Status: original specification, 2026-09-26. Current M7 reconciliation is in [release-manifest.json](release-manifest.json) and the [release checkpoint](../m7-release-checkpoint.md). The historical implementation-status descriptions below describe the initial test-design state; they are not current application status. Product scope comes from [product.md](../product.md) and [architecture.md](../architecture.md). This suite prepares the approved delivery workflow without running live model, Jira, Bitbucket or infrastructure tests.

**Execution follow-up:** [The offline results](results-2026-09-26.md) now record 950 real OMP identity bindings, 57 scripted profile turns, passing recovery/transport contracts and five failed contract checks. The full Crewroom workflow/browser cases remain unimplemented; lower-level experiments do not certify them.

The original suite contains 14 scaffold tests, a sanitized snapshot of 50 configured OMP model selectors, three complete role/model profiles, 87 specified behavior cases and 12 specified end-to-end scenarios. Their specification alone is not a passing test result.

## Test inputs and coverage map

- [suite.json](suite.json): 87 cases, each with a requirement, priority, tier, action, expected result and evidence. All are marked `specified-not-implemented`.
- [scenarios.json](scenarios.json): 12 synthetic tickets with complete workflows and case references.
- [model-profiles.json](model-profiles.json): 19 logical roles, three assignment profiles and negative routing inputs. Coordinator and delivery reporter are planned roles; the other 17 exist in the scaffold catalog.
- [OMP catalog fixture](fixtures/omp-model-catalog-2026-09-26.json): exact locally configured provider/model selectors and declared metadata, with secrets and endpoints omitted.
- [Model inventory research](../research/omp-model-catalog-for-simulation.md): sources, freshness and limits of the inventory.
- [Existing executable tests](../../tests): Jira parsing/import validation, role-file persistence/validation and sample isolation.

| Area | Cases | Main questions |
| --- | ---: | --- |
| Intake and requirements | 8 | Does refresh preserve local work? Are brief versions and repository selection explicit? |
| Role selection and customization | 7 | Are specialists selected by need? Can roles be inserted without changing existing assignments? |
| Model routing | 12 | Does the exact assigned model reach the process and provider request? Are defaults/fallbacks explicit? |
| External messaging | 9 | Are delivery, duplicate handling, causality, expiry and passive observation correct? |
| Process lifecycle | 9 | Can independent processes stop, survive peer failure and recover from durable state? |
| Scheduling | 5 | Are coding, specialist, writer and merge-dependency limits enforced? |
| Repository work | 6 | Is the operator checkout preserved? Do interrupted writes and base changes reconcile? |
| Evidence gates | 8 | Are criteria, checks and independent review current and attributable? |
| Jira/PR handoff | 7 | Are allowed updates reliable and uncertain writes reconciled? Does the human retain merge? |
| Operator interface | 6 | Are status, progress, steering and unavailable integrations represented accurately? |
| Containment and authority | 6 | Are secrets protected and unauthorized effects blocked? |
| Budgets and bounded execution | 4 | Do time accounting, retries, backpressure and history survive restart? |

The 14 current tests exercise `descriptionText`, `jiraImportSchema`, `RoleCatalog.read/update`, `roleSchema` and `sampleTasks`. They do not establish routing, persistence of runs, application security or integration behavior. There is no claim of complete line or branch coverage. The manifest maps intended functions to observable outcomes; add cases whenever implementation introduces a new public operation or failure boundary.

## Layers and what they prove

| Layer | Real components | Controlled components | Current status |
| --- | --- | --- | --- |
| Scaffold/domain | Current validation and role persistence functions; later workflow transitions and SQLite | Inputs, clock, IDs and routing proposals | 14 scaffold tests runnable; workflow domain planned |
| Process/adapter contracts | Separate OS subprocesses, disposable Git, durable stores and selected local mailbox implementation | Scripted runner protocol peers and fake Jira/Bitbucket servers | Retained mechanism/local mailbox experiments executed; Crewroom adapters remain unimplemented |
| Offline OMP integration | **A separate installed OMP process for coordinator and every active specialist**, external local mailbox, actual RPC/events and host tools | Loopback scripted model provider; synthetic credentials; fake Jira/Bitbucket | Identity/profile and selected lifecycle/fault contracts executed; see results for failures and limits |
| Browser journey | Crewroom server/UI, persistent workspace and browser | The same isolated model/provider fixtures | Planned; application entry points do not exist yet |

Fast protocol doubles help isolate controller bugs. They do not certify the OMP integration. Run the same applicable lifecycle/message/routing contracts against actual independent OMP processes before marking that layer passed. Do not substitute OMP internal team messaging for the approved external process arrangement.

The loopback model service must implement the wire formats OMP actually sends for the fixture's three configured API families: `openai-responses`, `anthropic-messages` and `google-generative-ai`. Start with the previously exercised Sol selector and responses API. Script text, tool calls/results, streaming chunks and failure events; reject unexpected requests instead of returning a generic success. Verify provider override behavior and request capture before broadening to the other API families. If a format is unsupported, report that integration gap and keep its cases unexecuted.

A scripted response can tell OMP to call a host tool or apply a known fixture patch. That tests tool dispatch and workflow behavior. It does not test whether the real model could discover the patch.

## Model routing matrix

The captured configuration contains 50 selectors across four provider entries and three API families. A configured entry is not proof of current credentials, tool support, service availability, useful output, latency or price. Only `aperture-openai/openai.gpt-6-sol` has previous Crewroom message-experiment evidence; even that is not a coding benchmark.

Exercise three separate decisions:

1. **Specialist selection:** Which roles does the accepted brief need? Use known scripted proposals, including invalid ones, and verify policy/scheduler decisions.
2. **Model assignment:** Which exact configured provider/model/effort does each assignment use? Validate capability requirements, preserve the decision and freeze its snapshot.
3. **Observed execution:** Which identity does OMP report, and which model/API does its actual request use? Compare both to the assignment before accepting work.

The binding matrix is **19 roles × 50 selectors = 950 identity combinations**. These now pass through the real OMP RPC boundary, without real model inference. Crewroom assignment/policy validation remains unimplemented. Preserve slash-bearing model IDs intact. Verify positive bindings separately from capability-negative dispatch cases.

Three profiles cover full journeys:

| Profile | Purpose |
| --- | --- |
| Baseline | Every role uses the previously exercised Sol selector. Confirms independence does not require a different model brand. |
| Heterogeneous | Roles use Sol, Sonnet, Gemini Flash, Luna and the configured Bedrock-compatible Haiku selector. Covers four provider entries and three APIs. |
| Rotated | Every role changes to another pool model. Detects hardcoded role/model coupling and accidental inheritance from the coordinator. |

Illustrative heterogeneous assignments are coordinator → Sol, implementer → Sonnet, reviewer → Gemini Flash, writer → Luna and delivery reporter → Haiku. Exact selectors and all remaining roles are in the profile file. These are test inputs, not quality-ranked production defaults; no user configuration is changed.

The matrix also covers unsupported thinking effort, text-only/image mismatch, unknown tool support, context/output budgets, removed/ambiguous models, wrong observed identity, provider errors and explicit fallback. Declared limits are preflight inputs with recorded provenance; they do not replace runtime error handling. The scaffold currently accepts only default/low/medium/high effort. Broader catalog efforts such as max/xhigh must not silently bypass that schema; expanding the UI/configuration is a separate implementation choice.

Resolve an empty model/default effort once and persist the explicit choice. Keep configured, resolved, runner-reported and provider-request identities distinct. Secondary OMP planning/summarization/model calls must be pinned, disabled or explicitly recorded under policy. A fallback requires an allowed compatible replacement and a new recorded attempt; lack of a policy means attention, not a guessed model swap.

A future JEV/System 1 selector can supply a proposal through the same interface. Test its malformed, unavailable and policy-violating outputs now using fixtures. Actual selection quality is outside offline evidence.

## Complete synthetic jobs

| Scenario | Work and expected outcome |
| --- | --- |
| E2E-01: small fix | One utility bug, actual local checks, independent scripted review and one fake PR ready for a human. |
| E2E-02: complex feature | UI, validation and docs criteria; selected specialists, one writer and heterogeneous assignments. |
| E2E-03: rework | A known failing patch and review finding block readiness; a corrected revision needs new evidence; third automatic repair is stopped. |
| E2E-04: interruption | Kill a recipient around message persistence/acknowledgement; separately restart coordinator/service; retain work without repeating effects. |
| E2E-05: supervision | Persist steering, insert a specialist, pause an active child tool, reload and resume under the accepted changed brief. |
| E2E-06: uncertain delivery | Fake Jira/Bitbucket commit a write then lose the reply; reconcile existing/absent/unknown outcomes. |
| E2E-07: queue/dependencies | Independent work waits for readiness; dependent work waits for observed human merge; Jira rank does not rewrite the queue. |
| E2E-08: routing failures | Unsupported settings, rate limits, missing models, allowed fallback and wrong/hidden model dispatch. |
| E2E-09: revision drift | Target movement and late hosted checks invalidate stale evidence; failure revokes readiness without auto-resuming work. |
| E2E-10: research | A requested design artifact with source evidence; no manufactured coding or PR gate. |
| E2E-11: authority | Malicious imported instructions become scripted unauthorized tool requests; control-plane permissions deny them. |
| E2E-12: limits/cancel | Budget survives waits/restart; cancellation stops future effects and retains artifacts; cleanup leaves foreign resources alone. |

Use synthetic `SIM-*` issues and small seeded local repositories. The coding fixture should include a real regression test, a deliberately wrong patch, a correct patch and a separately specified expected behavior. An optional tiny UI fixture adds real browser checks for the complex journey. Gold patches make workflow outcomes deterministic; test assertions must inspect resulting files/check results independently of the scripted agent's completion text.

the pilot repository is still the eventual pilot. Its checkout, Jira and Bitbucket are not the fixture. A later sanitized the pilot repository-shaped fixture may mirror check categories, but cannot establish the pilot repository compatibility or CI success.

## Harness contract and fault injection

Use the actual production domain/services wherever implemented. Avoid building a second simulated workflow that merely agrees with itself. Inject at narrow boundaries: clock, process launcher/runner adapter, model provider, external mailbox, Jira/PR clients and observable external events. Keep database, Git operations and local check commands real when those are under test.

The external message service remains behind its adapter. Run delivery contracts against the locally evaluated Relaycast version as well as deterministic doubles. Its current experiment stores some acknowledgement/idempotency knowledge in controller/service memory; the earlier happy-path probe did not prove recovery. Test mailbox restart, controller restart and recipient restart independently, then together. Do not assume exactly-once transport: assert durable deduplication and one logical side effect, with unresolved outcomes reported as uncertainty.

Use named fault points, not timed guesses:

- Intent persisted → request sent → service commits → reply lost → local acknowledgement persisted.
- Message read → durable inbox recorded → assignment applies it → result/effect recorded → transport acknowledged.
- Writer lease acquired → file changed → commit created → completion recorded.
- Pause requested → new dispatch blocked → agent interrupted → child process stopped → pause confirmed.
- Check/review captured → code/base/brief changes → stale marking → readiness recomputed.

At each relevant boundary, terminate the owned process and reopen its durable state. Save run, assignment, attempt, message, correlation and action IDs. Replacements get a new attempt; a late previous attempt cannot regain ownership. Simulate PID reuse with a process-registry fixture; never signal a real unrelated process to test it.

Use a fake clock for retry windows, message expiry, the 60-minute budget and waiting exclusions. Process tests use explicit readiness/events and bounded deadlines. Save random seeds and event order for generated interleavings. Suggested later stress targets are 100 seeded schedules and a bounded message burst; these are proposed budgets, not observed performance results.

Offline containment is a precondition for starting OMP: dedicated temporary configuration, synthetic credentials, no production tools/remotes, loopback-only fixtures, and a fail-closed network guard that also covers child processes and discovery/catalog requests. Environment variables alone do not establish isolation. Verify the guard with an intentional denied egress attempt before running integration cases. Use test-owned directories/config overrides rather than replacing the user's home or OMP configuration. If this boundary cannot be established, keep the OMP tier blocked and run only the isolated domain tests.

## Evidence and pass criteria

Each executable case should save:

- Case ID, execution layer, fixture/profile hashes, random seed and tool versions.
- Run/assignment/attempt identity and role/brief/project/model snapshots.
- Process/session identity and observed lifecycle/message events.
- Expected and actual provider requests, including action identities and side-effect counts.
- Git head/base, artifact hashes, actual command exit status/logs, review author and criterion coverage.
- Gate decision, unresolved uncertainty, cleanup result and any failing event trace.

Store generated evidence in ignored `.crewroom/test-results/<run-id>/`; export only sanitized summaries deliberately. Label provider/model observations as simulated when supplied by fixtures. Test-runner status and product workflow status are different fields.

For a case to pass, its expected observable outcome and absence of forbidden effects must both hold. A skipped test, missing implementation, unknown result or unavailable adapter is never a pass. P0 cases gate the first offline integration milestone; UI usability and generated stress coverage can expand after the minimal workflow is runnable. No duplicate dispatch, unauthorized write, stale-evidence readiness or hidden model substitution is acceptable.

The previously replayed 13 archived OMP-team tests are useful seeds for ownership locks, completed-phase continuation, check/review provenance, revision drift and uncertain publication. Their [saved report](../research/omp-contract-experiment.md) is historical evidence of the retained mechanisms, not proof that Crewroom has implemented those contracts. Port or wrap bounded mechanisms while retaining one workflow authority.

## Implementation order

Follow the [MVP build plan](../mvp-plan.md) for sequencing. The initial model matrix and the [14 Rust Mail/OMP/tmux checks](rust-mail-results-2026-09-27.md) have already run; preserve their evidence and rerun affected contracts when integrating or changing their boundaries.

1. **M1:** retain the scaffold regressions and add persistence/import/refinement tests through the real domain/API boundary. Bring up the missing application entry points and the first Playwright journey. A queued run remains visibly awaiting execution until its adapter exists.
2. **M2:** test actual application runtime adapters, strict model/effort policy, durable messages, and passive observation. Complete the unresolved receive/dispatch, supervisor-restart, surviving-writer, active child-tool pause, and bounded-recovery cases. A passing experimental message exchange alone does not complete LIF/MSG application coverage.
3. **M3–M4:** execute E2E-01/03 through the real workflow with a disposable Git repo, real commands, and controlled providers; add E2E-06 for lost Jira/PR replies. Reuse the same services in scripted tests and live adapters. Broaden model profiles and error/fallback cases where application policy is added.
4. **M5:** record the operator-selected live the pilot repository pilot separately. Offline evidence does not certify model reasoning, live credentials, latency/cost, or a real Jira-to-PR outcome. No live model/provider write is part of the default offline suite.
5. **M6–M7:** complete all twelve synthetic scenarios, queue/dependency, refinement/research, drift/feedback, usage/limit, and scope cases. Drive user flows through Playwright against the same services. Account for every behavior case with linked executed evidence; required blocked/skipped cases cannot be reported as passing.

Keep upstream characterization separate from selected-stack application acceptance. The old Relaycast restart failure remains valid history; Rust Mail has its own passing bounded retry contract. OMP's first-turn persistence and effort normalization require application policy and tests, not a rewritten historical result.

## Commands and observed results

Use Node 24 or later. On this host:

```sh
PATH=/opt/homebrew/opt/node/bin:$PATH npm test
PATH=/opt/homebrew/opt/node/bin:$PATH npm run build
```

During initial suite authoring on 2026-09-26, **14 tests passed; TypeScript checking passed.** The build then failed because `index.html` references missing `src/main.tsx`. `server/index.ts` is also absent, so application/browser acceptance is not runnable. The subsequent [execution report](results-2026-09-26.md) records the real OMP processes, scripted provider calls and recovery/fault checks. No live model, Jira or Bitbucket request occurred.

The [validation record](validation-2026-09-26.json) separates these executed checks from the unexecuted specifications. A local consistency check also verified catalog/profile identities, role coverage and scenario references; this validates the test data, not Crewroom behavior.

Use `python3 scripts/run-offline-suite.py` with a supported Node PATH to run the current release manifest and case/scenario accounting. Use `--check-manifest` for structural validation or `--only JOB,...` for a bounded diagnostic run. Partial coverage, failed or missing jobs, skipped tests and stale-source receipts keep the release result nonzero. The runner no longer executes the retired installation or historical Relaycast experiments.
