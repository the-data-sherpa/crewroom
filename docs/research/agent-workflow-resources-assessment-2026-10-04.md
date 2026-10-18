# What Crewroom can take from Poteto and Uncle Bob

Follow-up, 2026-10-12: the operator approved this assessment's additional work as [S9](../workbench-redesign.md#s9--verification-and-evaluated-learning). M8 S1–S8 are subsequently accepted on macOS; the [current continuation order](../status.md#continuation-order) prioritizes multi-harness development and retains S9. The recommendations below preserve the original research context.

Evaluated 2026-10-04 against Crewroom revision `5ced15090d3519609821cf405e86568f8a48e2bb`. This records the research baseline and recommendations; no runtime or user configuration was changed. The subsequent planning decision is recorded below.

**Subsequent planning decision, 2026-10-04:** the operator requested the ideas not already in M8 be added as [S9 — Verification and evaluated learning](../workbench-redesign.md#s9--verification-and-evaluated-learning). S9 now plans the three additions below and the bounded mutation-testing trial after S8. This assessment retains its original baseline and recommendations; implementation and validation have not begun, and adoption of any particular instruction revision still requires the existing operator decision.

## Assessment

The resources are useful, primarily as evidence for investing in verification and enforceable engineering constraints. They reinforce Crewroom's existing architecture. The best additions would help agents find the right verification path, turn recurring corrections into durable checks, and measure whether changes to agent instructions actually help.

The interesting outcome is less operator effort per trustworthy result. PR volume alone cannot establish that. The two Poteto resources describe the same practitioner's approach; they are not independent replications. Uncle Bob offers a complementary personal experiment, not a controlled comparison.

## What each resource adds

| Resource | Most useful material | How strongly to rely on it |
| --- | --- | --- |
| [Poteto interview](https://www.youtube.com/watch?v=MN9dGgmLyso) | Repeatable verification and deterministic tools ([16:14](https://www.youtube.com/watch?v=MN9dGgmLyso&t=974s), [20:42](https://www.youtube.com/watch?v=MN9dGgmLyso&t=1242s)); recurring corrections inform constraints and skills ([30:10](https://www.youtube.com/watch?v=MN9dGgmLyso&t=1810s), [1:00:35](https://www.youtube.com/watch?v=MN9dGgmLyso&t=3635s)). | Useful workflow experience. Reported scale is not an independently audited quality or cost result. |
| [Uncle Bob interview](https://www.youtube.com/watch?v=zcLPGC-tvgk) | Deterministic quality feedback ([05:35](https://www.youtube.com/watch?v=zcLPGC-tvgk&t=335s)); focused agents with coordination costs ([19:17](https://www.youtube.com/watch?v=zcLPGC-tvgk&t=1157s)); dependency constraints ([26:06](https://www.youtube.com/watch?v=zcLPGC-tvgk&t=1566s)). | Useful experiments and counterexamples. His numerical thresholds and agent sequence need local evaluation. |
| [Poteto's recorded X talk](https://x.com/poteto/status/2102050467505430555) | Maintained application-control tooling plus a feature map (09:19–12:30); architecture and static checks prevent recurring mistakes (15:45–18:52; 36:23–37:09). | Most concrete source for an incremental verification improvement. The [original captions](https://video.twimg.com/subtitles/amplify_video/2101938030122868736/0/WgLRQsQNJRcgqv1a.vtt) make the timestamps inspectable. |

Full automatic captions for all three were retrieved and read. This was transcript analysis, not a frame-by-frame audit of demonstrations. Source details, timestamps, metadata and limitations are retained in the [YouTube notes](video-source-notes-2026-10-04.md) and [X talk notes](poteto-talk-source-notes-2026-10-04.md). Raw transcripts remain outside the repository.

The X post says 2,500 PRs while the recording's captions say 2,000. The interview clarifies that the count includes maintenance ([45:51](https://www.youtube.com/watch?v=MN9dGgmLyso&t=2751s)). Neither number supplies a baseline, defect rate, operator workload or cost. It should not become a Crewroom target.

## What is already covered

These are comparisons against current project records and selected code, not newly executed validation:

| Idea | Crewroom position | Implication |
| --- | --- | --- |
| Require proof before completion | Implemented revision-specific checks and independent review; [readiness](../../crates/crewroom-runtime/src/evidence/readiness.rs) rejects missing/failed checks and changed revisions. | Improve the evidence agents can obtain. Do not replace the gate with a playbook checkbox. |
| Specialized agents with explicit responsibilities | Accepted coordinator/specialist workflow, separate processes, bounded compatible assignments and one writer per worktree. [Architecture](../architecture.md) | Preserve scope and independence. Additional roles must justify their coordination cost. |
| Model choices by job | Approved M8 S5; not yet implemented at the inspected revision. [M8](../workbench-redesign.md#s5--model-jobs-and-budget-schema-change) | Validate available models locally. Copy no speaker's model ranking or fixed model names. |
| Reusable principles and task playbooks | Approved M8 S6/S8; not yet implemented. [M8](../workbench-redesign.md) | Source material for those slices, not a new roadmap. |
| Learning from corrections | Implemented operator-decided role/project-guidance proposals. [Learning](../../crates/crewroom-core/src/learning.rs) validates evidence references and applies approved revisions; M8 S7 extends the experience. | Add evidence about a proposed lesson's effectiveness and route mechanical failures to code checks. |
| Asynchronous human supervision | Existing pause/steering/recovery; M8 S3 will collect pending decisions in Needs you. [Product](../product.md), [status](../status.md) | Keep consequential decisions visible while routine execution proceeds. |

## Three additions worth considering

### 1. Map an operator's feature name to a reproducible verification path

**Our recommendation:** extend the existing scenario/journey records with a thin index that an agent can use cold. Each entry should connect the feature name, prerequisites, user action, expected observable result, existing command or harness, and evidence location. Reuse the [acceptance reconciliation](../testing/rust-native-acceptance-reconciliation-2026-10-04.json), [operation inventory](../native-workbench-operation-map.md) and existing scripts. The method inventory alone does not explain how an operator reaches a feature.

An initial entry could cover steering delivery: select a running session, send a uniquely identifiable instruction, observe its stored/received/applied states, and check the durable message record. Repeat across a reconnect without treating a terminal sentence as proof of receipt. A service-only check cannot certify the native interaction; a scripted provider cannot certify live delivery.

**Small experiment:** one supported native journey, one disposable instance, an agent using the recipe without bespoke setup coaching. Retain the revision, platform, commands, actions, results and any unavailable capabilities. Expand only if it removes setup ambiguity. The existing S2 live-terminal check remains open; this report does not satisfy it.

**Cost:** maintained recipes and app-control tooling can drift. Validate their entry points when the feature changes; avoid a second competing acceptance catalog. This adapts the X talk's feature-map technique. The official pstack [verification generator][verify] and [maintenance workflow][maintain] provide inspectable examples, but browser/CDP control does not itself drive GPUI.

### 2. Make repeated corrections produce enforceable checks

**Our recommendation:** when the same failure recurs, ask which layer can prevent it: domain/API design, types, static checks, behavioral tests, or guidance requiring judgment. Crewroom's existing learning path targets role text and project guidance; a structural fix should become scoped repository work with its own evidence, rather than being squeezed into a role edit.

Example: if a UI change collapses unavailable and passed checks into one successful state, the useful outcome is a domain contract and failing regression case. Repeating a truthfulness instruction is insufficient. First verify whether the existing gate already prevents the failure; do not create redundant enforcement.

**Small experiment:** inspect a few retained steering/review/repair records, select one repeated mistake with a real reproducer, and prove a proposed check rejects the bad case while accepting legitimate behavior. Keep the evidence-to-rule link. Role/principle changes still follow the operator's approval path and affect future assignments only.

**Cost:** simplistic checks create false positives and workarounds. Prefer one demonstrated invariant over broad style policing. This is supported by pstack's [correction workflow][correct] and [structural-learning principle][structure], not a reason to adopt every rule in the plugin.

### 3. Test role and playbook revisions as behavioral changes

**Our recommendation:** supplement M8 S7's provenance and approval with a small comparative experiment when a proposed revision materially changes behavior. Give old and proposed instructions the same representative tasks, with identical repository state, model/effort and tool capabilities. Keep the scoring criteria outside the workers' prompts; compare artifacts under neutral labels and retain observed tool use.

Start with failures Crewroom actually cares about: claiming a skipped check passed, missing a stale revision, unnecessary operator escalation, or failing to verify the real surface. Record success, false completion, repair cycles, operator interventions, duration and actual usage when available. Distinguish scripted contract cases from live model behavior. Repeat before generalizing from a single run.

**Small experiment:** one proposed role revision across a handful of known cases; keep it unadopted until the operator reviews the result. No new fleet or automatic promotion is required.

**Cost:** real model evaluations consume time and money; subjective judges can reward style over correctness. Use observable predicates first. The supporting source is the inspected pstack [eval playbook][eval], not a claim that the X talk described blinded evaluation.

## Useful cautions and practices to leave out

- **Keep the human merge gate.** Poteto describes agents merging and humans sampling later ([52:38](https://www.youtube.com/watch?v=MN9dGgmLyso&t=3158s)). Crewroom promises a reviewable PR for human assessment. Sampling can help evaluate the system; it cannot silently replace that contract.
- **Keep accepted requirements and history.** Martin treats his specifications as temporary ([42:38](https://www.youtube.com/watch?v=zcLPGC-tvgk&t=2558s)). Crewroom needs durable accepted requirements for Jira reconciliation, interrupted work and revision-specific evidence. Small tasks and durable requirements are compatible.
- **Use focused assignments without a mandatory factory line.** Martin's named agents illustrate separation of responsibilities. Crewroom already chooses specialists per ticket; automatically adding every role would impose latency and handoff cost without demonstrated benefit.
- **Measure engineering outcomes instead of ritual compliance.** Martin explicitly questions line-by-line human TDD discipline for agents ([33:56](https://www.youtube.com/watch?v=zcLPGC-tvgk&t=2036s)). For Crewroom, retain concrete regression proof and small verifiable work units; do not mandate an interaction ritual solely because humans use it.
- **Trial mutation testing selectively.** One small deterministic module could reveal weak assertions. Do not impose universal mutation/coverage/complexity thresholds from an interview. The experiment should measure useful faults found and runtime cost.
- **Do not import blanket comment deletion, automatic backlog intake, unlimited concurrency or pstack's runtime assumptions.** Recovery rationale can be essential documentation. Crewroom's operator queue, ownership limits and independent OS processes remain binding.

## Reuse decision and next step

Inspect and adapt selected pstack verification recipes, principles and evaluation ideas. Do not install its whole orchestration layer or replace OMP, tmux, Rust Agent Mail, the Rust daemon or GPUI based on these resources. The plugin depends on Cursor-specific agent calls, rules and control skills; its [README][readme] lists additional dependencies. Source availability is not a tested Crewroom runner contract.

Official source inspected: `cursor/plugins`, commit `e43c7ee26e0038c6c1fa8380dd34ce86ff94cb2a`, pstack `0.15.9`, MIT licensed. No plugin code was executed or installed. Preserve license/attribution if implementation later copies material. The earlier [OMP reuse assessment](omp-team-reuse-assessment.md) still governs reuse of Crewroom's existing workflow mechanisms; its retired installation remains inactive.

Continue the approved M8 order, retaining the outstanding S2 live check and S3 Needs you work. These findings can inform the content of S6–S8 without changing their scope. My first additional experiment would be the single native verification recipe, followed by one demonstrated correction-to-check improvement. Treat comparative role evaluation as a bounded follow-up proposal, not a newly approved release gate.

Validation of this research: source/timestamp inspection, local document/code comparison and document link/whitespace checks. No product implementation, role configuration, live agent run, provider delivery or application test suite was changed or newly validated.

[readme]: https://github.com/cursor/plugins/blob/e43c7ee26e0038c6c1fa8380dd34ce86ff94cb2a/pstack/README.md
[verify]: https://github.com/cursor/plugins/blob/e43c7ee26e0038c6c1fa8380dd34ce86ff94cb2a/pstack/skills/create-verification-skill/SKILL.md
[maintain]: https://github.com/cursor/plugins/blob/e43c7ee26e0038c6c1fa8380dd34ce86ff94cb2a/pstack/skills/maintain-verification-skill/SKILL.md
[correct]: https://github.com/cursor/plugins/blob/e43c7ee26e0038c6c1fa8380dd34ce86ff94cb2a/pstack/skills/correct/SKILL.md
[structure]: https://github.com/cursor/plugins/blob/e43c7ee26e0038c6c1fa8380dd34ce86ff94cb2a/pstack/skills/principle-encode-lessons-in-structure/SKILL.md
[eval]: https://github.com/cursor/plugins/blob/e43c7ee26e0038c6c1fa8380dd34ce86ff94cb2a/pstack/skills/poteto-mode/playbooks/eval.md
