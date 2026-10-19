# Crewroom product direction

Status: approved by the user on 2026-09-25 after all 26 interview questions and the simple/complex workflow review. The design interview is complete. [architecture.md](architecture.md) records the approved implementation direction and the initial runtime stack selected on 2026-09-27; remaining integration and provider contracts are explicit acceptance work.

## Purpose

**Direction extended 2026-10-12:** the operator requested Codex and Claude Code alongside OMP, with models assigned to specialists and Crewroom resolving the compatible harness. They also confirmed plan-aware intake/grilling, a programming pair, and task-dependent UI/documentation specialists. Follow the [multi-harness development plan](multi-harness-plan.md); MH2/MH3 execution and their measured capability limits are recorded in [status](status.md), separately from discovery metadata. Plan-aware Queue behavior is defined in the [MH5 intake contract](testing/mh5-intake-contract.md); validation state remains in [status](status.md).

**Delegation direction confirmed 2026-10-12:** use named assignments within the existing per-run coordinator, retaining one executing coding ticket and explicit operator Resume for PR-feedback work. The [accepted delegation decisions](multi-harness-plan.md#accepted-delegation-decisions) are implemented by MH5/MH6; the [MH6 receipt](testing/mh6-pair-2026-10-13.json) bounds the executed evidence. Project-wide orchestration, concurrent coding tickets and automatic feedback admission remain possible later expansions outside the current MVP. Implementation and platform limits are recorded in [status](status.md).

Crewroom is a personal local application for directing specialized agents through software work taken from Jira. It keeps Jira current as work proceeds and produces a reviewable PR supported by credible evidence. The operator can inspect progress, intervene when work stalls or needs direction, and continue after interruptions.

On 2026-10-02 the operator selected a native Rust/GPUI Kit workbench and a Rust workflow service. The [migration plan](native-migration.md) preserves the accepted workflows and separate supervised processes. The accepted native migration and its bounded validation are recorded in the [native release record](release-native.md). Native launch is the repository default; the validated browser release remains an explicit compatibility and rollback reference.

On 2026-10-04 the operator deprioritized VoiceOver for this personal release. Screen-reader compatibility remains optional and unverified. Actual Japanese input-method composition was an additional migration compatibility check, beyond the accepted ordinary-keyboard and narrow-layout requirements; it also remains unverified and does not block this release. Existing accessibility labels, ordinary keyboard navigation, Unicode text handling and draft protection remain part of the implementation and its tested workflow.

The workflow is Jira → coordinator → selected specialist assignments → validated PR → human review/merge. The coordinator remains responsible throughout: it interprets the request and repository context, chooses specialists, reconciles their outputs, routes rework, and handles the operator's steering. A simple ticket uses fewer assignments; a complex ticket can branch into research/design work and multiple validation responsibilities while retaining the same quality gates.

The user has tried other tools without finding a suitable workflow. Their previous OMP team worked reasonably well, but they could not effectively steer it. Crewroom's value depends on making specialist work observable and manageable while keeping routine execution automatic.

## Scope

- One operator, with no planned shared-team product.
- Multiple repository configurations; one target repository per ticket.
- Repository-based coding and standalone research/design outcomes.
- No live infrastructure changes in the initial scope.
- Optional standalone Explore and Grill actions remain available. Queue reuses a current accepted implementation plan; otherwise it starts bounded coordinator-only scoping. Saving an idea starts no agent. Plan acceptance releases the original request under the existing queue and launch gates.
- Specialists selected according to the work. The operator can create reusable role definitions and insert specialists before or during a run. Mid-run insertion creates an explicit assignment at a safe coordination point; resulting changes can require renewed validation. Editing a role definition does not silently change an existing assignment.
- An initial model assignment for selection, with JEV or a System 1 model raised as possible later selection aids. No router or model identity has been selected.
- A delivery reporter maintains Jira progress, decisions, blockers, and evidence summaries. A technical writer maintains repository documentation, guides, and technical explanations. Both roles are customizable.
- Jira status, milestone/blocker comments, and PR/evidence links can be updated automatically. Rewriting Jira descriptions or acceptance criteria requires an explicit operator instruction.

## Operator control

After Run, Crewroom proceeds through specialist selection, implementation, validation, PR creation, and routine Jira updates within the agreed requirements. It asks the operator about consequential ambiguity and scope changes. The operator retains the final review/merge decision.

The operator supervises the team through one coordinating conversation and sees specialist progress. Pause, resume, and cancel are explicit controls. Pause stops new assignments and safely interrupts active work; the UI shows pausing until stopping is confirmed. Cancel ends further execution while preserving changes, evidence, and existing PRs. Stored, received, and applied steering remain separate facts.

Clarified by the operator on 2026-09-26: the coordinator and every specialist must run in individual processes that can be watched and monitored from outside the runner. Expose each agent's live output, tool activity, process state, and sent/received messages directly in the workbench. Separate conversations inside one OMP process do not meet this requirement. Agent communication uses an external, observable message service; the [selected runtime](architecture.md#selected-runtime-technologies) assigns that responsibility to Rust Agent Mail. Inspection does not require direct specialist steering.

Recovery preserves requirements, decisions, progress, and existing work. It should resume an existing usable agent session where possible and may use a replacement specialist when necessary. A replacement does not erase history or make stale evidence current. Allow at most two automatic repair/replacement cycles for a blocked assignment, then ask the operator. Escalate immediately if ownership or an external action's outcome cannot be established. Reconcile PR/Jira actions before retrying them.

Each run has a configurable active-execution limit, initially 60 minutes. Waiting for the operator or hosted CI does not count. Reaching the limit requests a safe pause with progress preserved. Show available usage and cost accurately, distinguishing estimates and unavailable values; enforce a monetary ceiling only when reliable accounting supports it.

## Work ownership and scheduling

Each coding effort uses an isolated branch/worktree based on the repository's current target. Continuing an existing branch or PR requires an explicit selection and ownership check. The operator's working checkout remains untouched.

The first release executes one coding ticket at a time, with up to three specialists doing compatible work and one writer per worktree. The next independent ticket may begin when the preceding PR is ready for review; dependent work waits for verified merge. Local execution priority is independent of Jira board order.

For the pilot, read tickets directly from the configured Jira source and explicitly assign the selected ticket to the pilot repository. Execute only work the operator queues. Automatic backlog selection and automatic repository mapping are outside this pilot.

During a Jira outage, continue existing work against its accepted requirements, retain pending updates, and show incomplete synchronization. If refreshed requirements conflict with the accepted brief, pause affected execution for an operator decision.

## Evidence required for coding outcomes

Every coding result needs acceptance-criteria coverage, actual results from the repository's required checks, and an independent review by a separate agent that did not implement the change. The evidence identifies the exact code revision covered. Failed or unavailable required checks prevent a ready-for-review claim.

When the target branch changes, mark affected readiness stale. Refresh the Crewroom-owned branch using the repository's rebase policy and rerun review/checks before restoring readiness. Resolve routine conflicts within accepted scope; ask the operator when requirements or ownership become uncertain.

Keep PR feedback visible and associated with its ticket. The operator selects Resume before agents address reviewer comments on the same owned branch/PR. Preserve history and validate the new revision. Newly failing required checks revoke readiness immediately, independently of whether execution has resumed.

## First useful milestone

Deliver one real Jira ticket through a reviewable PR, using the minimum persistent interface needed to demonstrate supervision and interruption recovery. This supersedes the handoff's earlier plan to complete a disconnected workbench before live integration.

Selectively reuse useful OMP team mechanisms and tests after validating them against the new workflow. The [reuse assessment](research/omp-team-reuse-assessment.md) identifies durable ownership, evidence gates, completed-phase continuation, and publication reconciliation. The old installation remains inactive; its entire engine and historical policies have not been adopted.

the pilot repository is the first proving ground. Following the bounded experiments, the operator selected OMP execution, tmux visibility, and Rust Agent Mail messaging for the initial runtime. Complete their production supervision/recovery integration and provider onboarding before claiming the live milestone works. The architecture links the passing evidence and outstanding contracts.

The representative Jira ticket, provider connection details, and model identities are onboarding inputs. The milestone's acceptance cases are in the architecture document; the actual ticket supplies its coding criteria. A minimal milestone still needs truthful status, saved refinements and steering, and evidence tied to the work it covers.

The [MVP build plan](mvp-plan.md) sequences the current scaffold through that first live proof and then the complete accepted personal workflows. It records implementation dependencies and exit gates without changing these product requirements.

## Constraints

- Jira status, Crewroom workflow state, and agent activity are separate facts.
- Imported Jira fields and local refinements are distinct; importing again preserves local work.
- Queued work, executing agents, stored messages, and acknowledged delivery remain distinguishable.
- Review and test evidence identifies the relevant revision. Terminal silence does not establish success.
- User role definitions are reusable; project instructions constrain each assignment.
- The workflow remains independent of a particular runner. Runner integration requires a real contract test.
- Harness versions (operator decision 2026-10-19, FOR-69): for Claude Code, Codex and OMP, any release newer than the oldest verified version, including a new major, may list models and run agents once a metadata-only compatibility probe passes for that exact executable. The verified list remains the baseline. The UI shows `verified` separately from `compatible (probed <version>)`; older, unparseable and probe-failing versions are refused with the failed check and a remedy. Nothing falls back to another harness or to free text. FOR-71: a reviewed known-broken list shipped with Crewroom refuses a listed release with its recorded reason even when the probe passes; the probe never triggers login, hooks, MCP servers or IDE setup; and resume and replacement re-probe a harness that changed since its recorded probe, holding the run with the reason on failure.

See [CONTEXT.md](../CONTEXT.md) for terminology and [discovery.md](discovery.md) for the decision record and final confirmation status.

## Programming pairs and staged contributions

Coding begins with the run coordinator translating accepted work packages into named responsibilities. Each package has a writer and a read-only navigator, explicit documentation ownership and a separate final reviewer. UI expertise is included for UI work; diagnosis is selected for a stated problem. Future stages are visible as plans, alongside actual assignment and process activity.

The coordinator reconciles concrete navigator concerns, reports and artifacts before closing a package. A documentation or UI contributor that must edit receives an explicit writing handoff before final freeze. A checkpoint preserves work without declaring a candidate ready. All changes, including documentation, precede revision-bound checks and independent review. The conversation shows pending delivery, blockers and transfer progress separately from completion. Implementation and executed acceptance are tracked in [status](status.md).
