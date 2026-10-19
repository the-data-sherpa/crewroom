# Crewroom

Crewroom is a personal workbench for directing specialized agents through work from Jira to a reviewable change with credible evidence. This glossary records terms settled in the project instructions and design interview.

## Language

**Operator**:
The single person who directs work in Crewroom and reviews its results.
_Avoid_: Team account, shared workspace member

**Ticket**:
A work item whose requirements and progress are tracked in Crewroom, whether created locally or imported from Jira or GitHub Issues.
_Avoid_: Run, agent assignment

**Project**:
One local checkout that Crewroom works in, together with its ticket source, checks, instruction files and branch prefix. The workbench groups tickets by project; a ticket without one is **Unassigned**. The service and stored data call a project a *repository*.
_Avoid_: Jira project, workspace

**Snooze / Settle**:
The operator's presentation of a ticket in the sidebar. A snoozed ticket sits on its project's **Snoozed** shelf until a time or a watched event (a PR review, CI finishing, a source status change) and then returns with the reason it woke; it returns at once if it needs the operator. A settled ticket sits on the **Settled** shelf until un-settled. Neither pauses, cancels or changes a run, workflow state or source status.
_Avoid_: Pause, cancel, close

**Target repository**:
The project assigned to a ticket as the destination for its coding work. In the workbench this is the ticket's project.
_Avoid_: Jira project

**Ticket source**:
Where a ticket came from: its provider kind (Jira, GitHub, or local) and its identity at that provider (site or repository, issue id, key, URL). A local ticket has no provider.
_Avoid_: Jira project, target repository

**Source status**:
The status of an imported issue in its provider's own workflow, shown with the provider name (for example "Jira In Progress" or "GitHub open"). **Jira status** is the source status of a Jira ticket. A local ticket has none and shows "Not linked".
_Avoid_: Agent activity, Crewroom workflow state

**Workflow state**:
The ticket's position in Crewroom's process for refining and carrying out work.
_Avoid_: Jira status, agent activity

**Agent activity**:
The observed execution activity of an agent assigned to work.
_Avoid_: Workflow state, Jira status

**No progress**:
A running run for which no agent output, peer message or repository checkpoint has been observed for a configured time. It is a separate flag computed from recorded events, not a workflow state, an agent activity or a failure.
_Avoid_: Stalled, failed, hung

**Refinement**:
Locally developed requirements, acceptance criteria, notes, and decisions that supplement a ticket's source content.
_Avoid_: Imported description

**Explore**:
An optional action for developing a ticket's idea and requirements before execution.
_Avoid_: Mandatory execution phase

**Grill**:
A conversation that challenges a ticket's requirements and assumptions. It can be requested separately or used to scope a queued idea without a current accepted implementation plan.
_Avoid_: Coding execution

**Intake request**:
The operator's queued intent to scope a repository job and proceed after accepting its implementation plan.
_Avoid_: Running agent, coding assignment

**Implementation plan**:
A versioned proposal of the goal, scope, decisions and work packages needed to satisfy a ticket's criteria. Operator acceptance records agreement with a particular requirements and repository basis.
_Avoid_: Run plan, launch details, assignment

**Work package**:
A named piece of proposed work with dependencies, expected outputs and validation obligations.
_Avoid_: Assigned specialist, executing agent

**Choreography**:
The run's allocation of accepted work packages to a programming pair and staged specialist responsibilities, with their reports and unresolved obligations.
_Avoid_: Implementation plan, running team

**Programming pair**:
A writer and an active navigator working on the same package. The navigator challenges the approach and evidence without holding write authority.
_Avoid_: Independent final review

**Package checkpoint**:
A preserved intermediate contribution that permits a named successor to request writing ownership before the final candidate is frozen.
_Avoid_: Candidate, completed ticket

**Documentation disposition**:
A package's recorded documentation outcome: a verified change or a reasoned decision that no documentation change is needed.
_Avoid_: Optional untracked follow-up

**Plan basis**:
The requirements and repository source against which an implementation plan was prepared and accepted.
_Avoid_: Model selection, execution evidence

**Run plan**:
A durable execution request with launch details, including selected roles and the captured requirements and accepted implementation plan.
_Avoid_: Running agent, completed work

**Run**:
The durable effort toward one selected ticket outcome, retaining history through pause, recovery, inserted specialists, and permitted continuation.
_Avoid_: Run plan, agent session

**Assignment**:
One specialist responsibility within a run, with inputs, expected output, dependencies, and an owner.
_Avoid_: Whole run, reusable role

**Attempt**:
One execution of an assignment; retry or replacement creates a new attempt while preserving previous evidence.
_Avoid_: New ticket, erased history

**Role**:
A reusable definition of an agent's responsibilities that the operator can create and edit, applied within the constraints of a particular project and assignment.
_Avoid_: Agent instance

**Specialist**:
An agent assigned a focused responsibility within the software development lifecycle, such as implementation, review, or validation.
_Avoid_: Entire delivery team

**Coordinator**:
The lead agent that interprets the ticket, selects and assigns specialists, reconciles their results, and handles operator steering throughout the work.
_Avoid_: One-time dispatcher, implementer

**Specialist selection**:
The choice of roles needed for a ticket's work.
_Avoid_: Model assignment

**Model assignment**:
The choice of model that performs a selected role's work.
_Avoid_: Specialist selection

**Billing route**:
Which account pays for a model offering's inference: subscription, API, or unknown when the harness login does not establish one. It is part of the offering's identity. Usage is reported per billing route and never totalled across routes: API use shows a list-price estimate, subscription use shows runs and tokens with no dollar figure, and unknown use (including assignments admitted before routes were recorded) shows no cost.
_Avoid_: Plan, tier, provider

**Subscription allowance**:
The remaining share of each rate-limit window (for example 5-hour and weekly) of a harness login on the subscription billing route, with its reset time and observation time, as the harness reports it. Reading it sends no prompt. It is visibility only: it never limits or pauses work. A window or login without a reported value is unavailable with its reason, never zero.
_Avoid_: Budget, quota cap

**Delivery reporter**:
A specialist responsible for Jira progress, decisions, blockers, and evidence summaries.
_Avoid_: Technical writer

**Technical writer**:
A specialist responsible for repository documentation, guides, and technical explanations.
_Avoid_: Delivery reporter

**Independent review**:
An assessment of a coding result by a separate agent that did not implement it, with findings tied to the code revision assessed.
_Avoid_: Implementer self-review

**Candidate**:
A completed coding revision identified by its base, commit, tree, requirements, project instructions and implementation attempts.
_Avoid_: Uncommitted writer progress, agent completion claim

**Locally validated**:
A candidate whose configured local checks and independent review support its unchanged revision. Publication and required hosted checks are separate facts.
_Avoid_: PR published, merged, hosted validation complete

**Ready for review**:
A coding result whose acceptance-criteria coverage, required checks, and independent review support the current code revision for human assessment.
_Avoid_: PR merely created, merged

**Paused**:
Work whose execution has stopped with retained progress available for continuation.
_Avoid_: Pausing, cancelled

**Pausing**:
Work for which a pause was requested but active execution has not yet been confirmed stopped.
_Avoid_: Paused

**Cancelled**:
Work whose further execution has been ended while its changes, evidence, and existing PR remain preserved.
_Avoid_: Deleted work, reverted changes

**Supervision**:
The operator's oversight of specialist progress, including intervention when work stalls or needs a change of direction.
_Avoid_: Direct specialist management

**Recovery**:
The continuation of interrupted work using retained context and progress.
_Avoid_: Starting the ticket over

**Steering message**:
A user's instruction intended to guide agent work; recording it and the intended agent acknowledging delivery are distinct events.
_Avoid_: Delivered instruction when receipt is unconfirmed

**Human review gate**:
The point at which a person assesses the proposed changes and their supporting evidence before deciding whether to merge.
_Avoid_: Automatic merge approval
