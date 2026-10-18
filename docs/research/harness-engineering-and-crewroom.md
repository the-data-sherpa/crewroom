# Harness engineering and Crewroom

Crewroom already provides much of the control layer needed to direct agents from a ticket to a validated change. Its next opportunity is to measure whether repeated use reduces the operator's corrective work while preserving or improving delivered quality.

This assessment accompanies the approved [M8 workbench slices](../workbench-redesign.md). The operator requested it on 2026-10-06 before continuing those slices. The improvement experiments below are recommendations for later work; they do not replace M8's acceptance cases or authorize automatic backlog selection, publication, or merging.

## Ideas from the talk

In [Harness Engineering: How to Build a Software Factory](https://www.youtube.com/watch?v=X6l4lpA0_NY), Dru Knox of Tessl distinguishes autonomy, automation, and quality. Autonomy concerns how much correction an agent needs; automation concerns how much work people permit it to perform without inspection. Successful unattended execution alone does not establish justified trust. See [2:58–4:30](https://www.youtube.com/watch?v=X6l4lpA0_NY&t=178s).

The talk separates fast development feedback, more exhaustive candidate checks, and a feedback process that improves future work. It also emphasizes recording human corrections so they can inform those improvements. See [the three loops at 6:43](https://www.youtube.com/watch?v=X6l4lpA0_NY&t=403s) and [the control plane at 11:33](https://www.youtube.com/watch?v=X6l4lpA0_NY&t=693s).

## Application to the current design

| Loop | Crewroom foundation | Opportunity |
| --- | --- | --- |
| Inner development loop | Contained specialist tools, coordinator feedback, configured checks, and bounded repair | Give agents fast, targeted feedback while they work. |
| Outer validation loop | Independent review, revision-specific evidence, hosted checks, and PR feedback continuation | Strengthen behavioral checks and actual product interaction. Expensive checks still gate readiness for the exact candidate. |
| Meta improvement loop | Durable steering, failures, review results, and approved learning proposals | Identify repeated causes across runs and evaluate whether changes prevent recurrence. |

The [architecture](../architecture.md) already supplies durable workflow authority, isolated work, observable independent processes, interruption recovery, and reconciled provider actions. Recorded steering is especially useful: a correction remains available after the conversation or process ends.

The existing learning implementation requires evidence and an operator decision before changing a role or project guidance. It retains versions and protects admitted assignments from later changes. Approval establishes authority to apply a change; it does not demonstrate that the change improves agent performance. See `crates/crewroom-core/src/learning.rs` and `crates/crewroom-core/src/roles.rs`.

Likewise, Crewroom's regression tests establish properties of the workbench and workflow service. They do not measure how reliably live agents solve representative tickets. The retained M5 delivery supplies one bounded live example; more varied live work is needed to assess intervention rates and delivered quality. The [status record](../status.md) distinguishes that delivery from scripted provider checks.

## Recommended improvement experiments

1. **Record a short run retrospective.** Capture which intervention was required and its cause: unclear requirements, missing capability, reasoning error, flaky validation, or environment failure. Keep new requirements and deliberate scope changes separate from agent mistakes.
2. **Make improvement proposals testable.** Connect the observed failure, the proposed role/tool/playbook change, and the result that would demonstrate improvement. Prefer a tool or executable check when instructions alone cannot reliably prevent recurrence.
3. **Compare versions on representative tasks.** Use a small set of past problems to compare successful outcomes, corrective interventions, elapsed time, and available usage/cost data. Record the model, role, playbook, repository, and candidate versions; keep unavailable cost data explicit.
4. **Measure operator effort per accepted result.** Separate corrective takeovers from planned decisions. Pair review effort and completion rate with escaped defects and acceptance outcomes. A design discussion is not an agent failure, and fewer comments can reflect less scrutiny rather than better work.

Start with a small operator-selected batch of real tickets. Use its evidence to choose the next improvement, evaluate that improvement, and retain the result. Do not turn every unusual run into a permanent instruction or change prompts solely because one attempt succeeded.

## Implications for M8

Finish the outstanding S2 terminal and S3 inbox checks before advancing. S4 makes role configuration understandable; S5 makes model resolution explicit; S6 supplies reusable principles; S7 strengthens proposal provenance and role history; S8 records playbook execution. These are useful foundations for future comparisons, but a role revision or completed checklist must never count as quality evidence by itself.

Keep model selection and reusable configuration separate from project constraints. Preserve the operator's ticket selection and merge authority. A larger number of automatically initiated PRs is not inherently a better result for this personal workflow.

The talk's suggestion that backlogs disappear is less persuasive for Crewroom. Faster implementation still leaves prioritization, ambiguous requirements, and limited review attention. The product outcome to pursue is lower corrective effort per accepted change, supported by stronger evidence.
