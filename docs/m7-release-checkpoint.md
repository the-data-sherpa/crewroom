# M7 release checkpoint

Historical checkpoint: M7 was in progress from accepted Slice F (`946555b`) when this work was recorded. M0–M7 were subsequently accepted on 2026-10-02; use the [final release record](release-0.1.0.md) and [current status](status.md) for completion and continuation. The original [87 cases](testing/suite.json) and [12 scenarios](testing/scenarios.json) remain authoritative. [release-manifest.json](testing/release-manifest.json) distinguishes mapped assertions from partial coverage; command receipts and source identities are written under the selected test-output directory.

## First reviewable increment

Commit `2d1f954` adds the manifest/runner and closes reproduced routing, pending-input and message-causality gaps. The preceding clean baseline passed 311 domain tests. The initial focused assertions passed 13 cases. The subsequent domain invocation passed 320 tests, but source edits began before it completed; the runner correctly rejected it as release evidence. It remains a diagnostic result. Initial provider/context checks passed in focused tests. Local Rust Mail downtime and two restarts preserved one logical delivery. These fixtures use simulated model/Jira/Bitbucket responses; the retained live pilot is unchanged.

Independent review found that a source-only receipt hash could incorrectly reuse a pass after a manifest or input-fixture change. The repair binds receipts to complete manifest/job definitions and hashes the case/scenario/model-profile/fixture inputs. Each candidate snapshot is retained under its content identity. Six harness assertions pass, including changed commands and fixture contents. Final integrated acceptance remains pending.

## Review repair and focused evidence

The provider guard inspects parsed content positions, accepts ordinary JSON-looking prose, and refuses unbounded file/media/context references. Opaque reasoning reserves the provider-reported output count from the response that produced it. A private per-assignment hash ledger preserves those counts across replacement attempts. Unknown opaque history remains held. Direct multi-turn gateway tests cover OpenAI, Anthropic and Gemini. The real OMP coding fixture passes 18 requests with OpenAI/Gemini replay; this installed runner drops the synthetic Anthropic redacted-only block, which is recorded as unobserved process replay.

Mailbox reads and keyed rate-limit rejections use bounded backoff within one overall 12-second deadline, including initialization. An ambiguous write stays pending. Idle observer bursts are capped at one poll per 1.5 seconds. Five killed-process receive windows against Rust Mail retain one keyed reply effect; external acknowledgement certifies durable acceptance, not completed agent work. Domain tests cover actual HTTP 429/503 responses, Retry-After, a stalled request and delayed initialization. The focused provider/usage/mail run passes 10 tests. The separate coding/delivery fixture passes 38 scripted requests. These are simulated model/Jira/Bitbucket boundaries, with real local workflow/runtime services.

## Coverage completion checkpoint

The manifest now maps **87/87 cases and 12/12 scenarios**, with no relabelled deferrals. Mapping is not acceptance: a complete frozen-candidate run remains required. Runtime identity includes binary hashes and versions for Node/npm/Python/Git/tmux/Docker/OMP/Mail, the OMP native add-on, and both effective and system process inspectors. Six accounting assertions pass; changed runtime identities invalidate receipts.

Focused execution, all under `.crewroom/test-results`:

- `m7-send4`: coordinator killed after Rust Mail committed a send but before its reply reached the application. Five scripted model requests; one external delivery, one causal reply, keyed replay, stale-token refusal and no surviving owned processes.
- `m7-drift2`: staged saved-filter UI/validation/guide work, one writer, Guide planning and assessment, independent unit and Chromium checks, initial publication, advanced-target refresh, new checks/review, same-PR update, current CI failure and late old-success refusal. 54 actual OMP requests; all four participant identities verified against configured models; no surviving processes. The initial drift fixture attempted publication too early; the application rejected it. The repaired fixture reacts to current readiness and requires current-attempt completion.
- `m7-authority8`: malicious imported Jira text and peer message, denied merge/deploy/path escape/fabricated review, real foreign research attempt/recipient isolation, foreign-origin refusal, controller/provider/child egress denial and listener inventory. Valid work still completes simulated delivery. `m7-authority7` correctly refused a second coding run before the isolation probe; the sentinel now uses the supported research-run path.
- `m7-matrix-interrupt`: SIGINT after actual OMP launch stops owned processes and produces a nonpassing interrupted matrix. The complete eleven-case provider matrix previously passed as `m7-matrix`.
- Owned-container integration: 5/5 pass (`/tmp/crewroom-m7-cleanup5.log`). A killed harness leaves durable ownership; recovery stops/removes only exact owned resources and retains receipts/output. Foreign PID/container/files, stale PID, forged labels and root/ancestor/journal symlinks are protected. Lost-delete response reconciles safely.
- Focused process/check contracts: 23/23 pass (`/tmp/crewroom-m7-process-checks.log`). Build passes (`/tmp/crewroom-m7-gaps-build.log`).

Standards review is clear after adding process-inspector fingerprints. Specification review completion/Guide/model-dispatch assertions are repaired; the real foreign-run probe now passes. The guarded fixture uses a private, non-setuid, ad-hoc-signed copy of the system `ps` because macOS disallows executing setuid `/bin/ps` inside this inherited sandbox; source/copy hashes are recorded. Production defaults remain `/bin/ps`.

M7.1 and M7 remain incomplete until the integrated candidate passes. Model/Jira/Bitbucket responses in these fixtures are scripted. Rust Mail, OMP, tmux, Git, Docker commands and browser checks are real. The retained live pilot remains unchanged.

## Subsequent gates

M7.2 must include stopped-command container removal that preserves durable check receipts, exact-ownership cleanup, supported runtime/configuration instructions, and tested backup/restore. A copy of only `workflow.sqlite` cannot restore worktrees, journals and evidence. Copies of owned worktrees also contain Git links into their original repository, so restore path and repository identity matter.

M7.3 must use a new data directory and role path, complete setup through configuration/API/UI without changing source, reopen retained work, then run another ticket. The supported live Jira loader currently uses ACLI/macOS Keychain and the Bitbucket adapter uses a server token or the configured pilot account. A fresh local scripted-provider test does not establish new live account access.

M7.4 records only achieved acceptance and remaining boundaries. It does not include hosted deployment, merging the retained PR, resuming the merged pilot, or publishing a package.

## Final budget and cancellation coverage repair

Checkpoint `f1f6ced` was frozen and its full run started, then deliberately interrupted during domain verification after final review found two required assertion gaps. Its report is nonpassing; no release acceptance is derived from it.

A new fake-clock reproduction proved that a running but settled team waiting for current hosted CI consumed 99,000 ms instead of retaining 500 ms. The repair persists a CI-wait interval only with a matching provider-observed PR/head/base and pending required status or pipeline, settled/stopped local attempts, no executing/uncertain command and no pending input/proposal. Actual model/tool execution, provider uncertainty and unresolved ownership keep charging. Optional skipped steps do not invalidate required CI waiting. Controller-confirmed stopped ownership closes a lost tool-exit interval; replacement and restart retain the prior usage. Confirmed operator/review pauses remain excluded. No elapsed time becomes check/review evidence.

Focused domain/lifecycle checks pass **9/9** (`/tmp/crewroom-m7-ciwait-green6.log`), build passes (`/tmp/crewroom-m7-ciwait-build3.log`), and real OMP coding/delivery passes **37 scripted requests** (`.crewroom/test-results/m7-ciwait-delivery1/report.json`). The new active-tool Cancel variant passes **1 request**, records an interrupted receipt and retained progress artifact, remains terminal after restart, removes the exact owned container and leaves no owned process (`m7-active-cancel1/report.json`). Both retained scoped reviews are clear. Manifest case rows also explicitly link cross-run/merge denial and committed-send crash to their execution jobs.

The diagnostic recovery worktree was archived after moving all 3,398 evidence files to `.crewroom/test-results/m7-recovery-retained`; its `relocation.json` preserves the original location. No live-pilot state was changed.

The `38130c3` full run exposed three DEL-04 compatibility failures before acceptance: publication-only Jira fixtures omit hosted observation/revision fields. The new wait classifier now returns false before reading identities when no observation exists. All 17 affected Jira/runtime/lifecycle assertions and build pass (`/tmp/crewroom-m7-ciwait-compat.log`, `/tmp/crewroom-m7-ciwait-compat-build.log`); scoped review is clear. The interrupted full run remains nonpassing, with no receipt reused as acceptance.
