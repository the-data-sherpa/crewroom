# Crewroom discovery

Status: all six rounds and questions 1–26 answered, 2026-09-25. Product-policy frontier is empty. the pilot repository is the first proving ground. The user approved the shared design on 2026-09-25: “I like it. I approve”. The design interview is complete and implementation may proceed. The initial runtime technology decision was subsequently approved on 2026-09-27, as recorded below.

## Runtime selection, 2026-09-27

After testing Rust Agent Mail and the independent OMP/tmux arrangement, the operator asked to add the validated technologies to the architecture. The selected initial stack is OMP 18.3.1 through per-agent RPC hosts, tmux 3.7c for hosting and visible activity, and standalone Rust Agent Mail v0.3.36 through HTTP MCP for durable external messaging. Crewroom retains workflow authority and its planned local workflow store. See [ADR 0005](adr/0005-use-omp-tmux-and-rust-agent-mail.md), [the selected architecture](architecture.md#selected-runtime-technologies), and [14 passing contract checks](testing/rust-mail-results-2026-09-27.md).

This resolves the initial runtime choice left open by the interview. No fallback or custom mailbox was needed. The evidence covers bounded mailbox/process behavior with scripted providers; production supervision, safe child-tool pause, strict model/effort policy, general recovery, and live Jira-to-PR delivery remain acceptance work. It does not select permanent role models, approve a different product scope, or reactivate the retired OMP team.

## Process clarification, 2026-09-26

After the successful single-process OMP message experiment, the user stated: “I don't want a single process. I want individual agent processes that I can watch,” and explicitly rejected OMP's internal messaging for Crewroom. Separate OS processes for the coordinator and each specialist, external monitoring of their progress, and an externally observable message service are now requirements. This settles the topology left open in the communication research; it preserves steering through the coordinator and leaves the transport implementation undecided. See [ADR 0004](adr/0004-observe-independent-agent-processes.md) and [the architecture contract](architecture.md#agent-processes-and-communication). The previous test remains valid within its stated scope but does not validate the required topology.

## Reading this document

[HANDOFF.md](../HANDOFF.md) records the scaffold and validation status. [product.md](product.md) records accepted product direction, and [architecture.md](architecture.md) records the approved implementation direction. This document records the completed interview and the inputs still needed for technical validation. [CONTEXT.md](../CONTEXT.md) contains the established vocabulary; unresolved meanings stay here until settled.

The user accepted the narrow live Jira-to-PR milestone in place of the handoff's earlier disconnected-workbench delivery order. All numbered policy recommendations have now been accepted. The architecture and domain boundaries were approved together; runtime choices are resolved by the stated experiments.

## Evaluation

The handoff is useful as an implementation recovery document. It distinguishes existing code from intended behavior, records validation limits, preserves prior choices, and explicitly separates queued work from live agents. Its rules about imported fields, local refinements, message delivery, and revision-specific evidence are valuable constraints.

The interview sharpened the product purpose: one operator directs specialized agents through portions of the SDLC and validation, starting from Jira, updating Jira as work proceeds, and producing a PR with a reviewable change and credible evidence. The user described a fine-tuned software factory suited to their own workflow. The accepted operating policies and first-milestone acceptance cases now make that purpose concrete.

The proposed persistent workbench can validate storage and navigation. On its own it cannot validate agent coordination, useful steering, independent review, recovery from interruption, or trustworthy PR readiness. The user therefore accepted a narrow live workflow as the first useful milestone, with the minimum persistent interface needed to prove those behaviors.

The user clarified that supervision should expose specialist work and progress and allow intervention when work stalls or needs direction, without requiring direct specialist steering. They accepted one coordinating conversation, explicit pause/resume/cancel controls, preservation of work, and bounded recovery. The runner experiment must establish how to deliver those behaviors accurately.

The original role list preceded a definition of when a role was needed or what authority it held. The accepted design now selects roles by need, permits operator-created roles and inserted assignments, retains project constraints and mandatory quality gates, and escalates consequential ambiguity to the operator.

## Scaffold verification

Read-only inspection during this interview confirmed the handoff's non-runnable scaffold description. The existing TypeScript check passes and the installed top-level dependency listing matches the handoff. Application behavior has not been validated.

- Startup still references the absent server entry point, and the HTML references the absent frontend entry point. See [package.json](../package.json) and [index.html](../index.html).
- The execution shell selects Node v22.22.3, below the package's minimum of 24. After approval, `/opt/homebrew/opt/node/bin/node` was verified as v26.9.0; select that supported runtime explicitly for Crewroom.
- In [shared/types.ts](../shared/types.ts), the ticket's project is a string without repository or checkout identity. A task revision identifies ticket content; code revision and review/test evidence are not modeled.
- The Jira importer in [server/jira.ts](../server/jira.ts) has no board, sprint, epic, hierarchy, or column mapping. Preserve the configured source's organization/order as returned by the live integration; do not invent unsupported mappings. The source-specific fields and transitions are onboarding/adapter facts to establish.
- [server/roles.ts](../server/roles.ts) enables all 17 default roles and prevents disabling triage, implementer, and reviewer. These are scaffold policies to evaluate, not evidence that the user accepted a mandatory team composition.
- The local [NTM fit assessment](archive/ntm-fit-assessment.md) and [workflow gate research](archive/ntm-workflow-gates.md) report source inspection. Real launch, message acknowledgement, gate enforcement, and interruption recovery still require a runner experiment. Their external sources were not revalidated in this interview.

## Existing OMP team work

The user requested selective reuse of useful mechanisms from the retired OMP team. Review [the reuse assessment](research/omp-team-reuse-assessment.md) before designing workflow persistence, recovery, evidence gates, or provider publication. Source inspection found implemented mechanisms and focused regression cases; this changes the technical starting point from a new design to an evaluation of existing components and gaps.

The archived 0.4.0 implementation launches OMP through its CLI and structured output, without terminal text injection. It includes completed-phase continuation and explicit handling of uncertain interrupted actions. It does not establish the supervision and automatic recovery experience requested for Crewroom. Reuse may mean extracting code, adapting contracts, or preserving tests. The user has selected OMP/Aperture for initial runner evaluation without adopting the whole retired engine.

The archive remains inactive. This review makes no new claim that its historical live trials or test suites pass in the current environment. The reuse request adds implementation evidence; the subsequent answers to questions 4–6 settle product scope separately.

## Specialist routing findings

The [routing reuse assessment](archive/routing-reuse-assessment.md) distinguishes the archived request classifier, classic coordinator guidance, and JEV's next-action/continuity advice. None establishes dynamic planning over Crewroom's user-created roles. JEV remains a possible later adviser, with no current adoption or model identity selected.

Use **role** for a reusable definition the operator authors and **specialist** for the assigned agent. The current role catalog only updates existing IDs, so creation and insertion are new required behavior. Round 4 settled the delivery-reporter/technical-writer responsibilities and the separate Jira write policy.

## Inherited decisions and constraints

These are established in the handoff or project instructions, rather than decisions newly made in this interview:

- Local browser application.
- Jira status, Crewroom workflow state, and agent activity remain separate facts.
- Local refinements and steering survive changes to imported Jira content.
- Reusable roles belong to the user; project instructions constrain assignments.
- Evidence identifies the revision it covers; terminal silence does not prove correctness.
- A human reviews the proposed changes at the merge gate.
- Workflow remains independent of NTM; an adapter requires validation against a real runner.
- The retired OMP team installation remains inactive. Selective reuse of its code and tests is explicitly requested; OpenRig was declined.

The handoff's suggested storage, UI, and integration sequence are proposals. They are not elevated to accepted architecture here.

## the pilot repository pilot findings

Private repository observations were removed for publication. Crewroom requires separate PR-created, validation-pending and ready-for-review states, with evidence bound to the candidate revision and execution environment.

## Design tree

Only questions whose prerequisites are settled belong in the current interview round. A dependency below means the child waits for its parent decision; listing a branch does not select its answer.

| Decision branch | Resolve first | Decisions it unlocks |
| --- | --- | --- |
| Primary payoff | Settled: specialized SDLC work and validation from Jira through a reviewable PR, with Jira updates | First useful milestone; measurable success; scope exclusions |
| Intended operator | Settled: personal use by one operator, with no planned team scope | Personal configuration and project boundaries; shared-user requirements excluded |
| Prior failure to prevent | Settled as a need: effective steering; the cause of the earlier difficulty remains unknown | Required steering interactions; an acceptance scenario for operator control |
| Work scope | Settled: repository-based coding and standalone research/design outcomes; no live infrastructure changes initially | Refinement outputs; role selection; representative acceptance scenario |
| Repository scope | Settled: multiple repository configurations, one target repository per ticket | Repository onboarding and identity; work isolation |
| Steering interaction | Settled at product level: observe specialists and intervene on the overall work without direct specialist steering | Coordinator interaction; acknowledgement; stalled-work remediation; scope-change handling |
| Interruption recovery | Settled: preserve progress, reconcile ownership/effects, resume or replace, bounded cycles | Runtime contract and fault-scenario validation |
| Routine authority | Settled: automatic progress within agreed scope, decisions requested for consequential ambiguity or scope changes, human merge gate | Exact Jira writeback; retry limits; refinement readiness |
| Refinement contract | Settled: Explore/Grill optional; direct Run allowed | Readiness criteria; outputs; approval of changed requirements |
| Team composition | Settled: select specialists by need; operator-created roles and insertion required | Selection mechanism; assignment contracts; insertion timing; reviewer independence |
| Documentation roles | Settled: delivery reporter for Jira/history; technical writer for repository documentation | Role definitions and assignment outputs |
| Specialist insertion | Settled: before and during runs, explicit assignment and renewed validation as affected | Safe coordination points; dependencies; snapshots |
| Jira write scope | Settled: automatic status/comments/links; explicit instruction for requirement changes | Workflow mapping; conflicts; failure handling |
| Quality baseline | Settled: criteria coverage, required check results, separate non-implementer review, exact code revision | Environment-specific evidence; failure gates; stale evidence |
| Pilot runner | Settled: evaluate OMP/Aperture first; integration unvalidated | Current transport contract test; steering/recovery capability; model availability |
| Pilot repository | Settled: the pilot repository | Actual project instructions; Jira/provider mapping; representative ticket; CI requirements |
| Recovery continuity | Settled: resume usable session or replace specialist while preserving work/history | Retained context; retry bounds; evidence freshness |
| First useful milestone | Settled: narrow live Jira-to-PR path with supervision and interruption recovery | First repository/ticket; provider/runner validation; acceptance criteria |
| Run lifecycle | Settled: coordinating conversation, pause/resume/cancel, preserved artifacts and truthful acknowledgements | Approved run/assignment/attempt vocabulary in the glossary |
| Revision and evidence | Settled requirement: exact code evidence, separate source/local requirements and role snapshots | Runtime provenance and stale-evidence validation |
| Repository ownership | Settled: isolated branch/worktree; explicit selection of existing work | Provisioning and ownership verification |
| Recovery limits | Settled: two repair/replacement cycles, immediate escalation for unresolved ownership/effects | Tested retry and escalation behavior |
| Queue and concurrency | Settled: one executing coding ticket, three compatible specialists, one writer; independent next work after readiness, dependencies after merge | Scheduling implementation and dependency observations |
| Jira failure behavior | Settled: continue accepted brief, pending writes visible, conflicts pause affected work | Adapter reconciliation and source-conflict tests |
| Jira intake | Settled: direct source read, explicit the pilot repository assignment, operator-queued work only | Actual source/ticket selection during onboarding |
| Target branch changes | Settled: refresh owned branch, stale readiness, repeated review/checks, scope/ownership escalation | Tested rebase and evidence invalidation |
| PR feedback | Settled: operator Resume admits feedback work on same branch/PR; failures revoke readiness | Provider observations and renewed evidence |
| Resource configuration | Settled: configurable 60-minute active limit, excludes human/CI waits, safe pause, honest usage/cost | Durable time accounting and supported monetary enforcement |

## Round 1: answers

1. **Primary payoff:** Specialized agents perform pieces of the SDLC and validation on work taken directly from Jira. Crewroom should produce a PR, update Jira as work proceeds, and deliver a reviewable change with credible evidence. The user has tried other projects and has not found one that suits their workflow. This answer establishes a broader delivery purpose than simply reducing manual coordination; it does not yet settle automatic actions or specific Jira writeback rules.
2. **Intended operator:** The user alone, with no expected scope beyond a single operator. Recorded in [ADR 0001](adr/0001-single-operator-scope.md).
3. **Prior failure to prevent:** The earlier OMP team worked reasonably well, but the user could not effectively steer it. They invoked it through OMP and are unsure whether their usage contributed. Effective steering is a product requirement; the interview has not established that OMP or its team implementation was technically incapable of it. The operating controls were subsequently settled in question 19.

## Round 2: answers

**Steering and recovery — clarified before the round:** The user does not want direct specialist steering. They want visibility into what specialists are doing and their progress, plus a way to remediate stalled work or redirect it. They also want Crewroom to pick up interrupted work. This supersedes the earlier recommendation to offer both lead and direct specialist conversations. The coordinating conversation, controls, and recovery policies were subsequently accepted in questions 10, 19, and 20; actual runner guarantees still require the contract experiment.

4. **Work scope — accepted:** Support repository-based coding and standalone research/design outcomes, with research, documentation, and validation as part of coding work. No changes to live infrastructure initially; that would require a separate scope decision.
5. **Repository scope — accepted:** Support multiple repository configurations, with each Jira ticket targeting one repository. Coordinated multi-repository tickets are outside the initial scope.
6. **Routine authority — accepted:** After Run, Crewroom can choose specialists and proceed through implementation, validation, PR creation, and routine Jira updates within agreed requirements. Ask the operator about consequential ambiguity or scope changes. Retain the human review/merge decision. Jira write scope and recovery bounds were subsequently accepted in questions 14 and 20. This is product behavior for Crewroom, not an instruction to start agents or publish anything during this interview.

## Round 3: answers

7. **Refinement contract — accepted:** Explore and Grill are optional. The operator can send a Jira ticket directly through Run. Material ambiguity still follows the previously accepted escalation rule; no mandatory Grill approval is added.
8. **Team selection — accepted:** Select specialists by need. The user expects an initial model assignment and raised JEV or a System 1 model as possible later selection aids. No model identity or routing implementation is selected. The operator must be able to create reusable roles and insert specialists as needed. Insertion and mandatory evidence were subsequently settled in questions 13 and 15; the architecture describes proposed assignment records.
9. **Documentation/Jira responsibility — completed by later answers:** A specialist can document in Jira and/or the repository. The user requested documentation and technical-writer specialists and the ability to build and insert them. The two role responsibilities and Jira field policy were subsequently settled in questions 12 and 14.
10. **Recovery continuity — accepted:** Preserve work and history, resume the original usable session when possible, and allow a replacement specialist to continue when necessary. Recheck evidence affected by subsequent changes.
11. **First useful milestone — accepted:** Prove one real Jira ticket through a reviewable PR with supervision and an interruption/recovery demonstration. Build the minimum persistent interface it needs and reuse validated OMP mechanisms where suitable. This supersedes the earlier disconnected-workbench delivery order.

## Round 4: answers

12. **Documentation role boundaries — accepted:** A delivery reporter maintains Jira progress, decisions, blockers, and evidence summaries. A technical writer maintains repository documentation, guides, and explanations. Both are customizable roles.
13. **Specialist insertion — accepted:** Insertion is available before Run and during an active run. Mid-run insertion creates an explicit assignment at a safe coordination point; its findings can require changes and renewed validation. Editing the reusable role definition does not silently alter an existing assignment.
14. **Jira fields — accepted:** Automatically update status, milestone/blocker comments, and PR/evidence links. Rewriting descriptions or acceptance criteria requires an explicit operator request.
15. **Quality baseline — accepted:** Every coding result requires acceptance-criteria coverage, actual required check results, and an independent review from a separate non-implementer agent, tied to the exact code revision. Failed or unavailable required checks prevent a ready-for-review claim. Repository rules determine applicable checks.
16. **Pilot runner — accepted:** Evaluate the existing OMP/Aperture setup first through a bounded contract test for launch, progress, interruption, and continuation. Keep the workflow independent and the retired team installation inactive.
17. **Pilot repository — accepted:** the pilot repository, located at `/Users/crewroom/Projects/Git/conductor`. The repeated copies of this answer are a single decision, not additional work requests. Inspection is read-only during the interview.

## Round 5: answers

18. **Work ownership — accepted:** Each coding effort gets an isolated branch/worktree based on the repository's current target, while the operator's working checkout remains untouched. Continuing an existing branch or PR is an explicit selection with ownership checks. See [ADR 0002](adr/0002-isolate-owned-coding-work.md).
19. **Supervision controls — accepted:** One coordinating conversation for steering, plus pause/resume/cancel. Pause stops new assignments and asks active work to stop safely; pausing remains visible until stopped execution is confirmed. Cancel ends further execution while preserving changes, evidence, and any PR. Stored, received, and applied steering remain distinct.
20. **Recovery bounds — accepted:** Reconcile saved progress and uncertain external outcomes before repeating work. Allow at most two automatic repair/replacement cycles for the same blocked assignment, then ask the operator. Escalate immediately when safe ownership or an external outcome cannot be established.
21. **Queue/concurrency — accepted:** One coding ticket executing at a time, up to three specialists doing compatible work, and one writer per worktree. An independent next ticket may start once the prior PR is ready for review; dependent work waits for verified merge. Local execution priority remains separate from Jira board order.
22. **Jira outage or changed requirements — accepted:** Continue work on an already accepted brief during a Jira outage, preserve pending updates, and show incomplete synchronization. Reconcile writes before retrying. If refreshed requirements conflict with the accepted brief, pause affected execution for a decision.
23. **Jira intake — accepted:** Read Jira work directly from the configured source, explicitly assign the selected ticket to the pilot repository, and execute only work the operator queues. Automatic backlog selection and repository mapping are outside the pilot. The actual source and ticket remain onboarding inputs.

## Round 6: answers

24. **Target branch changes — accepted:** When the target branch changes, mark affected readiness stale, refresh only the Crewroom-owned branch using the repository's rebase policy, and rerun review/checks before restoring readiness. Resolve routine conflicts within accepted scope; ask the operator if scope or ownership becomes uncertain. Respect established execution limits and preserve unrelated work.
25. **PR feedback — accepted:** Keep the PR associated with the ticket and show review feedback. The operator selects Resume to address feedback; Crewroom continues on the same owned branch/PR, preserves history, and validates the new revision. A newly observed required-check failure revokes readiness even before the operator resumes execution.
26. **Time and cost limits — accepted:** Start with a configurable 60-minute active-execution limit per run; waiting for the operator or hosted CI does not count. Reaching the limit requests a safe pause with retained progress. Report available usage/cost accurately and distinguish estimates and unavailable values. Enforce a monetary ceiling only when reliable accounting supports it.

## Final shared-understanding review

All policy branches have been visited and accepted. The final review covers the product brief, implementation design, glossary, and ADRs together. It introduces no additional product-policy question. The requested grilling workflow requires shared-understanding confirmation before implementation starts.

During this review, the user clarified the sequence as Jira → coordinator → selected specialists → validated PR and requested simple/complex flow visualizations. The product and architecture now state explicitly that the coordinator stays responsible throughout the run, including planning, delegation, result synthesis, rework, and operator steering. After inspecting the simple and complex flows, the user explicitly approved: “I like it. I approve”. This fulfills the grilling workflow’s shared-understanding requirement.

Implementation starts with evaluation of the OMP supervision/recovery contract and archived reuse candidates, followed by the minimum persistent workbench and real Jira/Bitbucket path for the selected the pilot repository ticket. Onboarding inputs and actual contract results remain explicit work, not missing product decisions.

## Onboarding and validation inputs

These are explicit follow-up inputs/experiments, not silently assumed design choices: the actual Jira connection/source and ticket, role model identities available through the selected runner, current Bitbucket policy/access, an isolated the pilot repository test environment, and the live runner/provider contract results. They will be established during the selected first milestone. No credentials, exact provider transitions, or working recovery capabilities are inferred from historical code.

The approved run/assignment/attempt terminology is recorded in `CONTEXT.md`; `architecture.md` describes its implementation boundaries.

### Recovery scenarios for contract validation

These are cases to test the accepted behavior against, not verified guarantees of the candidate runner:

- A specialist process exits while other work is still active.
- The browser closes while the local service and agents remain alive.
- Crewroom or the machine restarts after uncommitted changes have been produced.
- A PR or Jira update may have succeeded just before the connection was lost; recovery must establish its outcome before deciding whether to repeat it.
- A recovered agent has stale requirements because the operator redirected work before the interruption.

## Terms accepted in the shared-understanding review

- Run/assignment/attempt distinction recorded in the glossary.
- Saved Explore/Grill requirements, criteria, decisions, and unresolved questions; Run escalates material ambiguity.
- Separate requirement, role/instruction, and code revision identities, all retained with applicable evidence.
- Use repository configuration and Jira project as distinct concepts; avoid the scaffold's overloaded project string.

## Documentation as decisions settle

Record settled terms immediately in the glossary. Update this decision tree after each round and preserve the user's answers. Write an ADR only for a consequential trade-off whose rationale would otherwise be lost. Keep the accepted product brief current, write the architecture document once its decisions settle, and confirm shared understanding before implementation resumes.
