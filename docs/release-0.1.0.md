# Crewroom 0.1.0 — local MVP validation

Release date: 2026-10-02. Local validation; no hosted deployment or automatic package publication.

M0–M7 are accepted. The installation acceptance checkpoint is `965a563`; this release record changes documentation only. Crewroom supports the accepted single-operator Jira-to-PR workflow, alternate refinement/research outcomes, independent agent processes, observable external messaging, steering, safe pause/cancel/recovery, a local coding queue and revision-specific checks/review. Human review and merge remain with the operator.

## Revisions and evidence

- M7.1 acceptance candidate `6b73854`: 87/87 behavior cases, 12/12 scenarios and 39/39 jobs. Domain 332/332; build passes; browser 31/31. [Case and scenario receipts](testing/m7-acceptance-2026-10-02.json).
- Operational implementation `5ea1b1b`: completed and interrupted command containers are removed with retained receipts; restart reconciles cleanup after paused/cancelled state was saved. [Operations](operations.md) describes supported configuration, shutdown, migrations, retention and same-path backup/restore.
- M7.2 final checks: **332/332 domain, 31/31 browser, 6/6 container/ownership integrations, build passed**. Whole-state restore verified 695 entries and preserved evidence, sessions and steering before a second completed run. [Operational receipts](testing/m7-operations-2026-10-02.json).
- Fixture reliability `f25a951`: Git commands no longer depend on caller editor settings, and the release runner defaults to a short output path. The affected rebase passes with hostile editor settings; six receipt-accounting tests pass.
- M7.3 fresh checkout `47de583`: `npm ci`, build, actual `npm start`, own-origin enforcement, clean shutdown and configured coding → backup/restore → subsequent research all pass. [Installation receipts](testing/m7-installation-2026-10-02.json). Its application source, lockfile and resolved dependency tree match the final full domain/browser candidate; those suites were not repeated inside the fresh checkout.

The original M7.1 invocation passed 38 jobs and rejected seven matrix variants whose tmux socket paths were too long. The unchanged source/runtime/manifest matrix then passed at a shorter path; the report retains the first failure. Later operational checks are separately revision-bound. This is combined evidence across the recorded implementation checkpoints.

## Supported runtime

Validated on macOS / Apple Silicon with Node 26.10.0, npm 11.19.1, Python 3.14.7, Git 2.56.0, tmux 3.7c, Docker 29.4.1, OMP 18.4.4 and Rust Agent Mail 0.3.36. The acceptance JSON retains executable hashes. Schema version is 26. Node 22 is unsupported; select the documented Node path before installation or tests. Runtime upgrades require the affected contracts to pass again.

## Boundaries

Private pilot details were removed during FOR-17 history sanitization. Original evidence is retained in a private backup. This placeholder is not executable validation evidence.

The validated installation is local and single-operator. General cross-platform/account provisioning, hosted deployment, automatic merge, NTM and automatic history pruning are outside this release. Restore uses the original absolute paths and includes repository Git metadata, Mail storage and private configuration. Provider credentials remain server-side. Unknown ownership or external outcomes stay held.

Model fallback is explicit and policy-bound; there is no hidden substitution. Unsupported media/remote inputs and unknown reasoning budgets stay unavailable. Actual token usage can be absent, and monetary cost remains estimated or unavailable rather than an enforceable dollar ceiling.

The existing build chunk-size and terminal-renderer warnings remain. Historical timing failures are retained in the progress record; successful final checks do not establish that every timing intermittent is eliminated.

## Start

Follow [Local operations](operations.md) for `npm ci`, build/start, configuration and recovery. The browser must use the printed loopback origin. A fresh workbench starts without provider configuration and accurately reports unavailable integrations.

## Outstanding enhancements

Future work includes portable provider-account onboarding, broader operating-system validation, supported backup relocation/export, selective history archival, reliable monetary-budget enforcement and further terminal/bundle optimization. These do not waive any of the accepted 87 cases or 12 scenarios. Subsequent product work should be selected by the operator; this release starts no new live run.
