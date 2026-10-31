# Software factory foundations and build path

Research date: 2026-10-31. Tracking: FOR-95. Review version: 2. Consensus status: complete.

The starting point is a small, dependable path from an understood user need to a verified change, a controlled release, and feedback from use. Expand that path when evidence identifies a constraint. A large tool catalog or a team of agents is not a prerequisite.

This is an independent research baseline for deciding what a software factory needs and in what order to establish it. It does not assess Crewroom's implementation, recommend a rewrite, or establish that it is overbuilt. Existing code and project design documents were excluded. A project issue-title inventory was read solely to track this request; it was not used as evidence for these findings. The later assessment should apply the criteria here before choosing what to keep, simplify, connect, or build.

## What a software factory is

**C01 — A factory combines people, decisions, reusable practices, and tools to deliver and support software repeatedly.** DoD's contemporary definition includes people, tools, processes, and value for an end-user community. A pipeline is one part of that system. Earlier software-product-line work emphasizes reusable assets for a related family of products and phased adoption. Neither meaning requires autonomous AI. [DoD overview](https://www.cloud.mil/devsecops/); [SEI adoption roadmap](https://www.sei.cmu.edu/library/software-product-line-adoption-roadmap/).

A newer, AI-native usage emphasizes non-interactive production. StrongDM's February 2026 account describes agents implementing software from specifications and scenarios without human code writing or review. It uses scenarios outside the implementation codebase and simulated third-party services to validate behavior. This is a practitioner's report, not a controlled comparison establishing general safety or productivity. Its useful contribution is independent outcome validation; eliminating human code review is a stronger design choice that this report does not assume. [StrongDM's account](https://factory.strongdm.ai/).

StrongDM's stated operating loop is an initial specification or other input, an end-to-end validation harness, and feedback into subsequent attempts. The transferable foundation is the quality of that validation loop. Our recommendation is to bound its authority, cost, and stopping conditions and evaluate it on representative work. Fully unattended production is a possible operating model, not a mandatory starting point. [StrongDM principles](https://factory.strongdm.ai/principles).

For this report, a *software factory* means the whole delivery system. A *factory application* helps operate some portion of it. CI/CD supplies integration and delivery mechanics; an internal platform supplies shared capabilities; an agent harness lets a model act through tools; an orchestrator coordinates work. These are related responsibilities, not interchangeable products. CNCF describes platforms as user-oriented, composable services that can sit over existing implementations. [CNCF platforms](https://tag-app-delivery.cncf.io/whitepapers/platforms/).

The factory analogy has a limit: software needs are uncertain and designs evolve. Repeat the reliable delivery practices while preserving discovery, engineering judgment, and the ability to change a mistaken requirement. Standardizing decisions that are not yet understood can industrialize the wrong outcome.

## The minimum foundations

The requirements below are an engineering synthesis of the cited sources. Their exact grouping and acceptance demonstrations are proposed criteria, not an industry certification or a universal architecture.

### A bounded purpose and accountable owner

**C02 — Choose a user, a useful class of changes, and a service boundary before choosing an orchestration architecture.** Write down who needs the result, what success looks like, who can prioritize work, and who supports the result. Also name who can accept risk, change scope, authorize release, and stop work. One person can fill several roles; roles do not require a corresponding agent or department.

The initial charter can fit on one page: intended user; supported change class; excluded work; input and output; acceptance evidence; tool permissions; operating owner; budget; and escalation route. Include maintenance, defects, and support in the work queue. DORA's culture guidance supports shared responsibility, information flow, and learning after failure. [DORA culture](https://dora.dev/capabilities/generative-organizational-culture/).

**Demonstration:** a real request can be selected or declined for a stated reason, and everyone knows who resolves an ambiguous requirement. An agent may discover ambiguity; it should not silently settle a consequential product decision.

### An inspectable work contract and acceptance evidence

**C03 — Define what makes a change acceptable before delegating it, and keep that definition under accountable control.** A work item needs an objective, relevant context, constraints, a small scope, acceptance examples, and the conditions that require escalation. Relevant context includes how to build and test, domain terms, and architectural constraints; a ticket alone may be insufficient.

For exploratory work, acceptance may mean a learning artifact and the decision it enables; define product acceptance before promoting an implementation. Keep agent-facing context under a named maintainer, with revisions and updates when the product or toolchain changes. Stale context creates rework and can invalidate an otherwise sound plan.

Turn the checkable parts into executable tests. Leave genuinely subjective judgments with named reviewers and explicit criteria. Record what was checked and which revision was checked. Changing acceptance criteria is a deliberate product decision, not an expedient way to make a failing implementation pass. Agent evaluation guidance distinguishes a completion statement from the actual environmental outcome. [Anthropic evaluations](https://www.anthropic.com/research/demystifying-evals-for-ai-agents).

**Demonstration:** a plausible but wrong implementation fails acceptance; a changed revision cannot inherit evidence that no longer applies. Test changes can be legitimate, but changes to the acceptance basis need review outside the implementation's own assertion of success.

Keep some evaluation scenarios outside the implementer's read/write reach where practical, while keeping the intended behavior explicit. This reduces direct tailoring to known checks; it does not prove correctness, and repeated evaluator feedback can erode a holdout's independence. Maintain and refresh those cases deliberately. StrongDM describes external scenarios as one such mechanism. [StrongDM's account](https://factory.strongdm.ai/).

### A complete delivery path with explicit handoffs

**C04 — Prove one narrow path through the whole delivery system; the application may own only a defined segment.** The proposed minimum loop is:

Need → acceptance → implementation → verification → release decision → delivery → observation → next decision.

For example, a factory application might end at a verified pull request. That can be a valid product boundary if a named existing process owns merge, release, operations, and feedback. It is incomplete evidence for a *whole factory* if nobody can show what happens after the PR. A missing custom deployment feature is not a gap when an existing delivery system supplies the capability and its handoff works.

DORA distinguishes being able to release on demand from automatically releasing every change. Human release authority is compatible with continuous delivery. [DORA continuous delivery](https://dora.dev/capabilities/continuous-delivery/).

**Demonstration:** follow a small representative change into actual intended use, identify its revision and delivered artifact, and obtain an observation that informs a subsequent decision. Staging can prove mechanics; it does not establish adoption or user value. Delivery may be a desktop release, a package, a service, or another supported artifact.

### A dependable engineering baseline

**C05 — Make implementation, verification, and delivery reproducible enough for the actual risk before increasing throughput.** Use version control, declared dependencies, documented environment setup, relevant automated checks, controlled configuration, and identifiable release artifacts. Keep changes small and integrate them regularly. Fresh environments should not depend on an operator's undocumented machine state. Google SRE describes repeatable builds, automated release mechanics, and small changes; DORA describes how small batches shorten feedback. [SRE canarying chapter, release-engineering principles](https://sre.google/workbook/canarying-releases/); [DORA small batches](https://dora.dev/capabilities/working-in-small-batches/).

**Demonstration:** another execution can build and check the same revision from documented inputs, a failed required check prevents promotion, and a reviewer can reproduce a failure. Exact byte-for-byte reproducibility is a stronger property to require where warranted, not something this report assumes every initial workflow already provides.

Limit active work to the capacity to review, integrate, and support it. Producing more patches can increase waiting work when review is the constraint. WIP limits and visible flow are established DORA practices; applying them to agent-generated work is this report's recommendation. [DORA work in process limits](https://dora.dev/capabilities/wip-limits/).

### Security and authority at real boundaries

**C06 — Security applies both to the software being produced and to the machinery producing it.** Start with a risk assessment appropriate to the work: sensitive inputs, permitted external actions, trusted dependencies, access scope, protected environments, release evidence, and vulnerability response. NIST SSDF provides lifecycle practices; SLSA adds supply-chain integrity but does not establish application correctness or secure coding. [NIST SSDF 1.1](https://csrc.nist.gov/pubs/sp/800/218/final); [SLSA scope](https://slsa.dev/spec/v1.2/about).

For agent execution, grant capabilities separately: reading, editing, running code, publishing, merging, deploying, and destructive administration. Instructions and tool output must not grant themselves authority. Enforce limits outside the model through tool mediation and execution isolation. Keep broad service credentials outside generated code's reach. Anthropic's Managed Agents account illustrates why separating execution from credential handling matters. [Managed Agents](https://www.anthropic.com/research/managed-agents).

**Demonstration:** in defined adversarial cases, malicious task or dependency content cannot authorize a forbidden action or redirect a permitted one to an unauthorized purpose, such as putting private data into a public PR or comment. Verify the enforced boundary and record the attack coverage and remaining uncertainty; passing examples does not establish immunity to injection. Permissions must be revocable, and approval must bind to the intended action and applicable revision. Retain evidence with access and retention rules; more complete logging is not automatically safer.

Reduce combinations of private-data access, untrusted input, and outbound channels. Where all are necessary, restrict data access and mediate publication by account, destination, operation, and payload policy. An allowed domain or a generic secret scanner is insufficient by itself. Anthropic reports an attack that sent data to an attacker's account through an allowed API domain. [Anthropic containment account](https://www.anthropic.com/research/how-we-contain-claude).

Treat operator attention as a foundation: approvals and alerts must be understandable, consequential, and sustainable at the intended volume. Establish policies for routine bounded actions and reserve case-specific decisions for the appropriate risks. Measure supervision and manual-recovery time. Anthropic reports approval fatigue in its products; an inattentive or unqualified approver is not a dependable safeguard. [Anthropic containment account](https://www.anthropic.com/research/how-we-contain-claude).

### Observable work state and credible recovery

**C07 — Make work recoverable, and distinguish remembered intent from confirmed external effects.** An operator needs to see what was requested, what is executing, what changed, what was verified, what is waiting, what succeeded externally, and what remains uncertain. The storage mechanism can be simple. A distributed event system is not a prerequisite.

Persist enough information outside a disposable agent context to identify work, attempts, evidence, approvals, external resources, and unresolved actions. Resuming a conversation and safely resuming an external action are different problems. AWS explains why retry safety depends on idempotency contracts, including request identity and handling of changed parameters. [AWS retry semantics](https://aws.amazon.com/builders-library/making-retries-safe-with-idempotent-APIs/).

**Demonstration:** interrupt a run after an external action succeeds but before its acknowledgment is saved. On restart, reconcile with the external system before retrying. If the outcome cannot be established, expose that uncertainty and require resolution. Also exercise cancellation, changed requirements, and obsolete approvals. A persisted log alone does not guarantee exactly-once effects. Recovery can be manual at first if the handoff preserves evidence and prevents unsafe replay.

Enforce time and attempt limits outside the model, plus token or spending limits where reliable metering and controls exist. Where billing is delayed or unavailable, state the limit's precision and use conservative execution caps. A limit-stop must preserve evidence and require an explicit budget decision before resuming; it must not silently start a new attempt with a fresh allowance.

Use fakes or simulators for dangerous recovery scenarios, with known behavioral limits. Contract checks and selected live integration checks are still needed because a simulator can faithfully reproduce the wrong assumptions. StrongDM describes simulated external services for high-volume and otherwise unsafe validation. [StrongDM's account](https://factory.strongdm.ai/).

### Operation and learning after delivery

**C08 — Delivery needs an owner, a health signal, a recovery route, and a feedback channel.** Decide how the user gets the change, how a regression becomes visible, who responds, and how fixes return to prioritization. A service may use staged rollout; a package or desktop app may use distribution cohorts, diagnostics, support, and version withdrawal. Choose mechanisms that fit the product. SRE's canary practice evaluates limited exposure before wider rollout. [SRE canarying](https://sre.google/workbook/canarying-releases/).

As in C04, existing teams, platforms, or distribution channels can own these responsibilities. A factory application needs a verified handoff to them, not its own release or operations machinery.

**Demonstration:** exercise recovery and retain the result, then use a user observation, defect, or operational finding to decide the next change. Database and irreversible external changes may need forward repair or compensation rather than rollback. “No incidents reported” is insufficient without a way to observe or receive failures.

## The build path

**C09 — Expand through demonstrated capabilities, with small increments and explicit exit decisions.** The following sequence is proposed, not a fixed maturity model. Security, usability, feedback, and recovery begin at the relevant first exposure and deepen with risk. An existing toolchain can already satisfy a stage. A supervised AI prototype can appear early; the sequence does not require constructing a new manual factory before trying AI.

| Stage | Work to establish | Evidence to leave the stage | Defer unless already justified |
| --- | --- | --- | --- |
| 0. Define the service | One-page charter, representative work, acceptance basis, existing delivery owners, risks and baseline | A real change has a clear outcome, authority boundary, support route, and measurable starting point | General architecture and broad feature backlog |
| 1. Complete a thin end-to-end path | Use existing development and delivery tools; connect manual handoffs where needed | A representative change reaches intended use; revision and artifact are traceable; one relevant recovery route is exercised, manually if appropriate; feedback produces a decision | Universal workflows and broad integration coverage |
| 2. Make the path dependable | Automate recurring friction; enforce checks; retain work state; clarify interruption, cancellation, and uncertain outcomes | Repeat the path across representative cases and injected failures without silent loss, unsafe replay, or invalid promotion | More simultaneous work than review and operations can absorb |
| 3. Establish bounded AI value | One agent or simple workflow, constrained tools, representative evaluation cases, existing approach as baseline | Accepted outcomes, human effort, elapsed time, cost, and failure handling meet predeclared local criteria | Broad autonomy based on a convincing demo |
| 4. Expand authority selectively | Add only demonstrated useful action classes or longer assignments | Re-evaluate changed permissions, revisions, recovery, adversarial inputs, and operator intervention at the new scope | A single unrestricted “autonomous” switch |
| 5. Reuse and scale where demand exists | Shared paths, additional consumers, optional specialists or parallel work | Benefit persists after integration, review, support, and coordination costs; another real consumer succeeds | A platform roadmap driven only by possible future consumers |

For a factory intended for AI-assisted delivery, Stages 2 and 3 should normally interleave. Introduce a bounded agent on the thin path, then harden the path as that agent actually executes it. Dependability evidence must cover its interruption, stale-state, and permission failures. Stage 1 establishes a relevant recovery route; Stage 2 broadens and repeats failure exercises. This is progressive assurance of the real path, not a requirement to finish a manual automation platform first.

Repeatability means representative variation and meaningful failures, not one fortunate run. Choose evaluation volume and thresholds according to consequences, variability, and available evidence; there is no supported universal “ten runs” or “95%” gate here. Record the decision to proceed and its residual risk. CNCF explicitly says higher maturity requires investment and is not a goal in itself. [CNCF maturity model](https://tag-app-delivery.cncf.io/whitepapers/platform-eng-maturity-model/).

### What to do first in practice

1. Select one valuable, low-consequence change class with an owner and objective acceptance examples. Keep a realistic path to broader work, but do not use it to expand the first demonstration.
2. Map the current human-and-tool route from request to use. Mark each handoff, wait, decision, and recovery owner. This is the first activity of the later assessment, not a claim that it was done here.
3. Complete a representative change with existing tools. Record elapsed time, active human time, review and rework, cost, and observed user result. A prototype using synthetic data can validate mechanics before real exposure.
4. Deliberately exercise the likely failure: failed acceptance, interrupted execution, or uncertain publication. Repair the smallest missing capability that prevents safe completion or diagnosis.
5. Trial a bounded agent assignment against comparable work. Keep the quality bar and account for correction effort. Expand only the capability that improves the actual constraint.

The first deliverable should be a working path with evidence and known limitations. A new scheduler, agent hierarchy, or management dashboard may contribute later, but is not the default starting deliverable.

## What AI changes

**C10 — Start with the simplest adequate execution pattern; a single agent is a useful default, not a rule.** Deterministic steps such as running tests and checking authorization should remain explicit. Use model judgment where interpretation or adaptation is needed. Anthropic recommends escalating complexity only when it improves the task, while distinguishing predefined workflows from dynamically directed agents. [Anthropic agent patterns](https://www.anthropic.com/research/building-effective-agents).

A small initial path can have a human owner, one coding agent, existing verification tools, and human acceptance. Multiple roles in the process do not imply multiple concurrent agents. Separate reviewers or specialized tools can be justified by the risk without building a general multi-agent framework.

**C11 — Evaluate the agent and its harness together before widening its authority.** Use representative tasks, repeated trials where variation matters, deterministic checks where possible, calibrated human judgment, and actual resulting state. Keep acceptance evidence independent of a model's confidence. Record model, instructions, tools, environment, revision, and evaluation version so later changes are comparable. [Anthropic evaluations](https://www.anthropic.com/research/demystifying-evals-for-ai-agents).

Begin with a small set of real tasks and observed failures, then grow it as gaps emerge. Grade the resulting state rather than requiring one exact sequence of implementation steps, while separately checking mandatory authorization and safety constraints. Inspect selected traces to verify that graders reward the intended behavior. Suite size is a local design decision, not a universal readiness threshold.

Keep two kinds of verification distinct: tests of the product being changed, and tests of the factory's behavior. A product suite can pass while the factory publishes twice, resumes obsolete work, bypasses review, leaks data, or reports a canceled run as completed. Test the workflow around the product, including its permissions and failure paths. Reassess a changed model or harness rather than assuming older evaluation results still apply.

**C12 — Add multiple agents when task structure and measured benefit justify the coordination cost.** Independent research or separated work may parallelize well; tightly coupled edits can require substantial reconciliation. Anthropic reports benefits for its multi-agent research system but also higher resource use and difficulties with interdependent work. That vendor report does not isolate agent count or establish universal coding productivity gains. [Anthropic multi-agent research](https://www.anthropic.com/research/multi-agent-research-system).

Compare against a simpler configuration with declared model choices and resource budgets. Count integration failures, review work, conflicting changes, and total operating cost. Role labels alone establish neither expertise nor review independence. Agreement between agents is a review signal, not proof of correctness; shared sources and assumptions can produce correlated mistakes. That limitation also applies to this report's cross-model consensus process; its error-reduction effect has not been measured.

## What to measure and what to reuse

**C13 — Measure accepted results and total delivery effort, not just generated activity.** Track whether users received the intended outcome, escaped defects and recovery, lead time and waiting time, human attention including rework, and operating cost including failed attempts. Add workload-specific measures only when they inform a decision. Use an observation window suitable for delayed defects and maintenance cost.

DORA currently describes five delivery measures: change lead time, deployment frequency, failed deployment recovery time, change fail rate, and deployment rework rate. Its lead-time definition starts at a commit; separately measure request-to-use time when discovery and queueing matter. These metrics are not a complete product-value score. [DORA metrics](https://dora.dev/guides/dora-metrics/).

DORA cautions against gaming metric targets, misleading comparisons across different services, competition between teams, and relying on a single measure. Use measures to improve a service over time. A small pilot normally supports descriptive learning; claim a statistical effect only when the sampling and analysis justify it. [DORA metrics](https://dora.dev/guides/dora-metrics/).

For a small pilot, a reviewed change record and a simple ledger can be enough. Include declined, failed, abandoned, and corrected attempts. Hold task mix and quality expectations as comparable as practical; record remaining confounding. Avoid claiming benefit from faster code production when review or support becomes more expensive.

**C14 — Reuse commodity capabilities and build the smallest missing product responsibility.** Source hosting, CI, identity, secrets, package storage, release tools, telemetry, and established agent runtimes are candidates for reuse. Evaluate their permissions, isolation, recovery contracts, exportability, licensing, support burden, and full lifecycle cost. Integration still has an owner. CNCF's platform guidance supports thin, composable interfaces over existing services. [CNCF platforms](https://tag-app-delivery.cncf.io/whitepapers/platforms/).

Ordinarily defer a universal workflow language, a broad provider catalog, an agent marketplace, elaborate role hierarchies, and dashboards unconnected to operator decisions. Keep the basic operator experience needed to understand state, inspect evidence, approve when required, stop work, and recover. Simplicity must remain usable.

Revisit custom harness work as models and runtimes change. Anthropic reports that model improvements rendered some of its earlier harness workarounds unnecessary. Prefer small integration boundaries over duplicating runtime internals when the available runtime meets the required contract. Reuse still requires verifying that contract. [Managed Agents](https://www.anthropic.com/research/managed-agents).

Exceptions matter. A stable product family, existing multi-team demand, isolation needs, or assurance obligations can justify shared assets early. SEI's product-line approach is relevant when those commonalities are real. The decision is based on demand and constraints, not a prohibition on abstraction. [SEI adoption roadmap](https://www.sei.cmu.edu/library/software-product-line-adoption-roadmap/).

## How to assess Crewroom afterward

**C15 — Assess demonstrated capability across the whole delivery arrangement, then assign each responsibility to its proper owner.** Freeze these independent criteria before inspecting the implementation. Read the accepted product and architecture contracts before recommending changes. Map app-owned capabilities, external systems, and human responsibilities separately.

First record the application's accepted boundary. For each foundation above, create a short record with: intended behavior; owner; present mechanism; observed evidence and revision; important failure case; operator attention and its sustainability at intended volume; consequence of a gap; smallest useful next action. Classify it as **demonstrated**, **partial**, **unverified**, **missing**, or **deliberately out of scope with a working handoff**. “Unverified” means evidence is absent; it does not assert the capability is absent.

Start with one representative end-to-end journey and its likely interruption. Read code afterward to explain the observed behavior and locate changes. Existing tests or screens are candidate evidence, not automatic proof of operational capability. Distinguish simulated integrations from live external effects.

Prioritize gaps by consequence and by whether they block the next useful delivery demonstration. Classify features as essential now, useful later, supplied elsewhere, or unsupported by current need. Preserve working mechanisms; remove or simplify only after understanding dependencies. Advanced features alongside an unproven basic path are a signal to investigate sequencing, not a sufficient reason to delete them.

## Evidence strength and remaining uncertainty

**C16 — The foundations have stronger support than any precise AI architecture, productivity forecast, or staged recipe.** Standards prescribe practices; practitioner reports describe mechanisms; observational studies identify associations; controlled experiments address particular interventions and populations. None validates this exact six-stage path as the universal optimum.

The primary AI productivity literature is context-dependent. Cui and colleagues report gains from coding-assistant access across company experiments, with larger gains among less experienced developers; METR's early study found a slowdown for experienced maintainers working in familiar repositories. METR's February 2026 update says later data are an unreliable estimate because of selection and time-measurement problems. It believes selection likely understates benefit and that speedups have probably increased since early 2025, while the magnitude remains uncertain. These concern different interventions and populations, and neither establishes the payoff of a fully autonomous factory. [Cui et al. study summary and preprint](https://www.microsoft.com/en-us/research/publication/the-effects-of-generative-ai-on-high-skilled-work-evidence-from-three-field-experiments-with-software-developers/); [METR initial study](https://metr.org/blog/2025-07-10-early-2025-ai-experienced-os-dev-study/); [METR methodological update](https://metr.org/blog/2026-02-24-uplift-update/).

Long-term maintainability, rare harmful actions, and multi-agent coding benefits under comparable budgets remain uncertain in this review. Vendor publications have commercial and selection biases. Regulated, embedded, disconnected, or safety-critical work may need stronger controls earlier. Set those requirements before adopting the proposed sequence.

This is a selective primary-source review, not an exhaustive literature review. The [source notes](software-factory-foundations-source-notes-2026-10-31.md) retain the wider historical context and source-status ledger. DoD passages were accessible through indexed official text but some direct fetches were blocked. The SEI roadmap and Cui study were read through their primary summaries; full underlying analyses were not independently audited. SSDF 1.1 is cited as final; the source notes distinguish the 1.2 draft. Proposed gates, examples, and assessment categories are our synthesis.

## Consensus record

Consensus was reached after two review rounds with Claude Code Opus 5.5 (`claude-opus-5-5`). In round 1, Opus accepted four findings and accepted twelve with addenda. The parent evaluated every decision and additional concern, revised the report, and returned reasoned acceptances, qualifications, and one rejected citation correction. In round 2, Opus accepted all C01–C16 findings and all additional resolutions without conditions. The parent accepted that final evaluation. No substantive disagreements remain.

The agreed substantive report is v2, SHA-256 `99934ef3a21bc644292715e85ef15bb80300fea71296e29395943659fe7287fb`. The reviewed source notes hash is `e18831a5349d8edb1c7d487a5f84574bce02b8805a24c14e66840d581f04d4dc`; the parent's reciprocal-response hash is `edb96d3f7a978beda23d330e08a883c15bf06114fa104dd566372f36ca804eb7`. Subsequent changes only finalize status and this consensus record; the substantive report is unchanged.

The [consensus decision ledger](software-factory-foundations-consensus-2026-10-31.md) records the objections, reasoning, responses, and final agreement. Consensus includes the report's uncertainties and does not establish empirical proof or independent validation of the findings.
