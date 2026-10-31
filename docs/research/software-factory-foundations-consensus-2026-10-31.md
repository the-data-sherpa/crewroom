# Software factory research consensus decisions

Tracking: FOR-95. Date: 2026-10-31. Status: complete consensus after two Opus review rounds and reciprocal parent evaluations.

This ledger records the reasoning used to revise the [independent report](software-factory-foundations-2026-10-31.md). The initial research and all review briefs excluded Crewroom code and pre-existing design documents. The parent read project issue titles only for task tracking. Raw review files and immutable report snapshots are retained outside the tracked repository.

The parent is GPT-6 Astra through Codex. The reviewer is Claude Code Opus 5.5, confirmed by the delegation runtime as `claude-opus-5-5`. Each review round is a fresh delegation with the previous report, review, and parent response supplied explicitly. The labels are **accept**, **accept with addendum**, and **reject**. A non-accept must state a reason. Agreement requires accepting the same final substantive report with no unresolved conditions; it does not establish empirical validity.

## Round 1 and parent response

Opus reviewed report v1, SHA-256 `9c13ba9d4c58a165b36584bb4078291179231963a9781397f65836f1ed4b61ff`.

Opus's verdict: **accept with addenda**. Across C01–C16 it recorded four accepts, twelve accepts with addenda, and no rejections. It also raised five omitted topics (U1–U5) and four citation notes (N1–N4). The following entries preserve each position and the parent's response; the parent did not interpret conditional acceptance as consensus.

| Finding | Opus position and reasoning | Parent evaluation and resolution in v2 |
| --- | --- | --- |
| C01: definition | **Accept with addendum.** Historical and DevSecOps meanings omit the recent AI-native model; verify StrongDM before citing it. | **Accept with addendum.** The omission matters for the intended reader. Parent read StrongDM's account, principles, and techniques directly. Added its non-interactive model, external scenarios, simulation, and validation loop. Narrowed the reviewer's wording: the primary account specifies no human code review; it does not justify assuming all review is performed by agents or all scenario validation by humans. Treat the model as a practitioner choice, not a universal requirement. |
| C02: purpose and owner | **Accept.** Bounded charter and responsibilities are useful without multiplying roles. | **Accept.** The proposal is proportionate and remains explicit synthesis; no change needed. |
| C03: work contract | **Accept with addendum.** Add protected acceptance cases and accommodate exploratory tasks. | **Accept with addendum.** Added learning artifacts as exploration acceptance and holdout scenarios where practical. Clarified that intended behavior remains visible and repeated evaluator feedback can weaken holdout independence. Hiding requirements or claiming protected tests prove correctness would overstate the mechanism. |
| C04: full path and app boundary | **Accept.** Correctly distinguishes factory obligations from custom app features. | **Accept.** Preserve the working-handoff criterion; it prevents turning every end-to-end need into product scope. |
| C05: engineering baseline | **Accept with addendum.** Claim is supported, but the SRE link label should identify the actual chapter. | **Accept.** Relabeled the link to the canarying chapter's release-engineering principles; no substantive change. |
| C06: security and authority | **Accept with addendum.** Forbidden-action tests miss harmful use of already permitted channels, including disclosure through a PR or comment. | **Accept with addendum.** Added permitted-capability misuse and primary evidence from Anthropic's containment account. Narrowed demonstrations to defined adversarial cases with residual uncertainty. Restricted publication by account, operation, destination, and payload policy; a domain allowlist or generic scanner is not a general defense. The reviewer's suggestion of simply adding a deterministic check is insufficient unless its actual boundary is specified and tested. |
| C07: recovery | **Accept.** Correct separation of conversation continuity and side-effect reconciliation, without requiring a distributed event system. | **Accept.** Preserve the finding. Additional budget and simulator details are resolved under U3 and U5. |
| C08: operations | **Accept with addendum.** Explicitly repeat that release and operations may be external responsibilities. | **Accept.** Added a C04 cross-reference and existing-team/platform ownership; this closes a plausible scope misreading. |
| C09: stages | **Accept with addendum.** For AI-focused delivery, hardening a manual path before introducing an agent can miss the actual failures; interleave Stages 2 and 3. Distinguish the first recovery demonstration from broader failure exercises. | **Accept.** Added explicit interleaving and agent-specific failure coverage. Stage 1 now exercises one relevant recovery route, manually where appropriate; Stage 2 broadens and repeats the exercises. This preserves safety prerequisites without creating a serial manual-platform project. |
| C10: execution pattern | **Accept.** Simple/default-one-agent recommendation is appropriately conditional. | **Accept.** Retain risk-justified specialized review without imposing an orchestration framework. |
| C11: agent evaluation | **Accept with addendum.** Give a small practical starting suite; judge outcomes and inspect traces. Suggested citing Anthropic's 20–50-task example. | **Accept with addendum.** Adopt small real-task suites and trace inspection, but omit the numerical example to avoid a de facto universal gate. Outcome grading must still check mandatory authorization and safety constraints; a correct final artifact can conceal an unauthorized intermediate action. |
| C12: multiple agents | **Accept with addendum.** Role names do not establish independence; apply correlated-error caveat to this consensus loop too. | **Accept with addendum.** Added both points. Did not adopt the reviewer's assertion that this loop reduces some errors: no experiment here measured that effect. Say it is a review signal with shared-source and assumption risks. |
| C13: measurement | **Accept with addendum.** Add DORA cautions about gaming, cross-service comparison, and single metrics; qualify small pilot statistics. | **Accept with addendum.** Verified DORA and added its cautions. Small pilots usually support descriptive learning; statistical claims require justified sampling and analysis, rather than being categorically impossible at a particular size. |
| C14: reuse | **Accept with addendum.** Fast-changing runtimes can make custom workarounds obsolete. | **Accept.** Added periodic reassessment and thin integration boundaries, conditioned on the reused runtime meeting the required contract. Vendor change is a reason to review a decision, not automatically replace working code. |
| C15: later assessment | **Accept with addendum.** Record the accepted product boundary and operator burden. | **Accept.** Added both to the evidence record. This helps identify excessive supervision and prevents imposing whole-factory responsibilities on a narrower app. |
| C16: evidence limits | **Accept with addendum.** METR's update also expresses likely increased gains and downward selection bias; Cui reports greater gains among less experienced developers. | **Accept.** Parent verified both primary summaries. Added these findings while preserving uncertainty about magnitude, populations, interventions, and fully autonomous factories. Neither proves a local effect. |

## Additional issues and citation decisions

| Item | Opus reasoning | Parent evaluation and resolution |
| --- | --- | --- |
| U1: AI-native meaning | Important omitted interpretation; same issue as C01. | **Accept with addendum**, as C01. StrongDM is represented on its own terms without importing its design rules as standards. |
| U2: operator burden | Approval fatigue can turn human review into rubber stamping or make supervision the constraint. Originally offered as synthesis. | **Accept.** Added sustainable operator attention to foundations and assessment. Parent additionally found first-party Anthropic evidence of approval fatigue. Use bounded authority and informative decisions; do not make every action a confirmation. |
| U3: runaway loops and cost | Naming a budget is insufficient without enforcement and a visible limit-stop. | **Accept with addendum.** Added external time/attempt limits and token/spend controls where reliable. Delayed or unavailable billing cannot support an exact claimed spend ceiling. Use conservative execution caps, preserve evidence, and prevent retries from silently resetting allowances. |
| U4: context maintenance | Agent-facing context becomes stale and costs effort to maintain. | **Accept.** Added an owner and revision-aware maintenance in C03. No special knowledge platform is prescribed. |
| U5: simulation | Simulators can safely exercise failures unsuitable for live systems. Optional extension. | **Accept with addendum.** Added the technique with fidelity limits and selected live/contract checks. Simulation supports recovery testing but cannot establish live delivery behavior by itself. |
| N1: SRE label | Link title does not match the chapter title. | **Accept.** Resolved under C05. |
| N2: Anthropic paths | `/research/` URLs resolve to engineering publications; no correction needed. | **Accept.** Retain URLs and describe evidence as engineering accounts. |
| N3: OpenAI date | Reviewer could not access the page and requested that the January 2026 web date be marked unverified or replaced by the guide's earlier original date. | **Reject the proposed date correction.** Parent directly fetched the cited web page, which displays January 15, 2026. The source notes now specify that this is the displayed web-page date, not an assertion about original guide publication. A reviewer's access failure is not contrary evidence; the clarification addresses the legitimate ambiguity. |
| N4: Managed Agents date | April 8, 2026 confirmed. | **Accept.** Existing April 2026 status is accurate. |

New primary sources checked for these resolutions: [StrongDM account](https://factory.strongdm.ai/), [principles](https://factory.strongdm.ai/principles), [techniques](https://factory.strongdm.ai/techniques), [Anthropic containment](https://www.anthropic.com/research/how-we-contain-claude), and [OpenAI guide web edition](https://openai.com/business/guides-and-resources/a-practical-guide-to-building-ai-agents/). Previously cited DORA, Cui, and METR passages were also checked. These sources support the bounded statements in the report; the proposed operating rules remain synthesis.

At the close of round 1, the parent accepted the revised v2 substantive findings. Opus had not yet accepted v2 or the reasoned modifications above. Round 2 evaluated the entire revised report, updated source notes, and these responses, including N3's rejection.

## Round 2 and final parent evaluation

Opus verified and accepted these exact snapshots:

| Artifact | SHA-256 |
| --- | --- |
| Report v2 | `99934ef3a21bc644292715e85ef15bb80300fea71296e29395943659fe7287fb` |
| Source notes v2 | `e18831a5349d8edb1c7d487a5f84574bce02b8805a24c14e66840d581f04d4dc` |
| Parent response to round 1 | `edb96d3f7a978beda23d330e08a883c15bf06114fa104dd566372f36ca804eb7` |

Its final verdict was **accept**, with no unresolved conditions. It accepted C01, C02, C03, C04, C05, C06, C07, C08, C09, C10, C11, C12, C13, C14, C15, and C16; U1, U2, U3, U4, and U5; and N1, N2, N3, and N4, including the rejection of its proposed N3 date correction. It rechecked StrongDM, Anthropic containment, and DORA small-batch claims against primary sources. The OpenAI web page remained inaccessible to Opus; it accepted the parent's directly observed page date and the distinction from original publication.

**Parent evaluation: accept each of those round-2 decisions.** Opus's reasons are consistent with the revised evidence and the parent's qualifications. No further substantive revisions are necessary. The findings retain bounded claims about holdouts, security controls, evaluation sizes, operator capacity, resource limits, and model consensus. Both sides accept the same report and source notes, including their stated uncertainty.

Opus left two optional observations, explicitly not conditions of acceptance:

- **O1: accept the observation, retain agreed wording.** StrongDM says scenarios are *often* stored outside the codebase. The report says the technique is used, not that it is universal, and separately recommends it only where practical. The parent agrees that this does not create a substantive conflict.
- **O2: accept the housekeeping observation.** The source-notes preamble now records its review-stage additions. DORA WIP and small-batch sources are cited directly in the report and were verified; their absence as separate rows in the broader source-notes ledger is not missing evidence. No new source entries were added after acceptance.

Opus explicitly accepted metadata-only finalization: update completion status, record actual round outcomes and reviewed hashes, and link this ledger. The parent made only those changes after the accepted v2 snapshots. No claim, gate, example, recommendation, or substantive citation changed. Code and pre-existing design documents remained outside the research and review scope.

The research and consensus are complete. Applying this baseline to Crewroom is the subsequent assessment, not a result of this report.
